//! A light-client scan of the Ironwood pool for one disposable wallet: its notes, whether they are spent, and their
//! Merkle witnesses, from the light wallet server's compact blocks. Ported from Zecured's
//! `signer/crates/zcash-ops/src/scan.rs` (73de9f909e270e8aea10c2b221905f62a6a4c4df).
//!
//! Witnesses use incrementalmerkletree's legacy API: a disposable wallet holds a handful of notes and is scanned from
//! its (recent) birthday, so this stays cheap.

use anyhow::{Context, Result, anyhow, ensure};
use incrementalmerkletree::{frontier::CommitmentTree, witness::IncrementalWitness};
use orchard::{
    Note,
    keys::{FullViewingKey, PreparedIncomingViewingKey, Scope},
    note::Nullifier,
    note_encryption::{CompactAction, IronwoodDomain},
    tree::{MerkleHashOrchard, MerklePath},
};
use zcash_client_backend::proto::{
    compact_formats::CompactBlock,
    service::{BlockId, BlockRange, PoolType},
};
use zcash_note_encryption::try_compact_note_decryption;

use crate::lwd::{self, LwdClient};

/// Depth of the Ironwood note commitment tree (the Orchard protocol's).
pub const TREE_DEPTH: u8 = orchard::NOTE_COMMITMENT_TREE_DEPTH as u8;
/// The Ironwood note commitment tree (legacy representation).
pub type IronwoodTree = CommitmentTree<MerkleHashOrchard, TREE_DEPTH>;
/// A witness of one note in the Ironwood tree.
pub type IronwoodWitness = IncrementalWitness<MerkleHashOrchard, TREE_DEPTH>;

/// Where a note is in the chain, in chain order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct NotePosition {
    pub height: u32,
    pub tx_index: u64,
    pub action_index: u32,
}

/// A note the wallet received.
#[derive(Clone)]
pub struct ReceivedNote {
    pub position: NotePosition,
    pub note: Note,
    pub scope: Scope,
    pub nullifier: Nullifier,
    pub witness: IronwoodWitness,
    /// The height of the block that spent it, if one did.
    pub spent_at: Option<u32>,
}

impl ReceivedNote {
    /// Its value, in zatoshi.
    pub fn value(&self) -> u64 {
        self.note.value().inner()
    }

    /// Its Merkle path to the tree's current root.
    pub fn merkle_path(&self) -> Result<MerklePath> {
        Ok(MerklePath::from(self.witness.path().context("a note witness without a path")?))
    }
}

/// The scan: the Ironwood tree after block `scanned_to`, and every note found so far.
pub struct ScanState {
    pub fvk: FullViewingKey,
    pub tree: IronwoodTree,
    /// The last block scanned.
    pub scanned_to: u32,
    pub notes: Vec<ReceivedNote>,
    /// The hash of block `scanned_to`, to notice a reorganisation (empty before the first block).
    last_hash: Vec<u8>,
    ivks: [(Scope, PreparedIncomingViewingKey); 2],
}

impl ScanState {
    /// A scan that starts after block `prior`, whose Ironwood tree is `tree`.
    pub fn new(fvk: FullViewingKey, tree: IronwoodTree, prior: u32) -> Self {
        let ivks = [Scope::External, Scope::Internal].map(|scope| (scope, fvk.to_ivk(scope).prepare()));
        Self {
            fvk,
            tree,
            scanned_to: prior,
            notes: vec![],
            last_hash: vec![],
            ivks,
        }
    }

    /// A scan from block `birthday`, from the server's Ironwood tree state after block `birthday - 1`.
    pub async fn from_birthday(client: &mut LwdClient, fvk: FullViewingKey, birthday: u32) -> Result<Self> {
        let prior = birthday.checked_sub(1).context("a birthday after block 0")?;
        let tree = lwd::tree_state(client, prior).await?.ironwood_tree().context("the Ironwood tree state")?;
        Ok(Self::new(fvk, tree, prior))
    }

    /// Scans every block after `scanned_to` up to `to`, with the Ironwood parts of their transactions.
    pub async fn scan_to(&mut self, client: &mut LwdClient, to: u32) -> Result<()> {
        if to <= self.scanned_to {
            return Ok(());
        }
        let block = |height: u32| BlockId {
            height: height.into(),
            hash: vec![],
        };
        let mut stream = client
            .get_block_range(BlockRange {
                start: Some(block(self.scanned_to + 1)),
                end: Some(block(to)),
                pool_types: vec![PoolType::Ironwood as i32],
            })
            .await
            .map_err(|s| anyhow!("GetBlockRange: {}", s.message()))?
            .into_inner();
        while let Some(b) = tokio::time::timeout(lwd::STREAM_MESSAGE_TIMEOUT, stream.message())
            .await
            .context("GetBlockRange: the stream stalled")?
            .map_err(|s| anyhow!("GetBlockRange: {}", s.message()))?
        {
            self.scan_block(&b)?;
        }
        ensure!(self.scanned_to == to, "the server sent blocks up to {} instead of {to}", self.scanned_to);
        Ok(())
    }

    /// Adds one block: marks the wallet's notes it spends, appends its note commitments to the tree and the
    /// witnesses, and keeps the notes the wallet's keys decrypt.
    pub fn scan_block(&mut self, block: &CompactBlock) -> Result<()> {
        let height = u32::try_from(block.height).context("a block height")?;
        let expected = self.scanned_to.checked_add(1).context("a block height")?;
        ensure!(height == expected, "expected block {expected}, got {height}");
        // A reorganisation replaced a block already scanned: the caller scans again.
        ensure!(
            self.last_hash.is_empty() || block.prev_hash == self.last_hash,
            "the chain was reorganised below block {height}"
        );
        // Everything the server sent is checked before any state changes.
        let mut actions = vec![];
        for tx in &block.vtx {
            for (action_index, a) in tx.ironwood_actions.iter().enumerate() {
                let nf = a.nf().map_err(|_| anyhow!("block {height}: an Ironwood nullifier that is not one"))?;
                let cmx =
                    a.cmx().map_err(|_| anyhow!("block {height}: an Ironwood note commitment that is not one"))?;
                let compact = CompactAction::try_from(a).map_err(|_| anyhow!("block {height}: a malformed action"))?;
                let position = NotePosition {
                    height,
                    tx_index: tx.index,
                    action_index: u32::try_from(action_index)?,
                };
                actions.push((position, nf, MerkleHashOrchard::from_cmx(&cmx), compact));
            }
        }
        for (position, nf, node, compact) in actions {
            for note in self.notes.iter_mut().filter(|n| n.spent_at.is_none() && n.nullifier == nf) {
                note.spent_at = Some(height);
            }
            for note in &mut self.notes {
                note.witness.append(node).map_err(|_| anyhow!("the note commitment tree is full"))?;
            }
            self.tree.append(node).map_err(|_| anyhow!("the note commitment tree is full"))?;
            let domain = IronwoodDomain::for_compact_action(&compact);
            for (scope, ivk) in &self.ivks {
                if let Some((note, _)) = try_compact_note_decryption(&domain, ivk, &compact) {
                    self.notes.push(ReceivedNote {
                        position,
                        nullifier: note.nullifier(&self.fvk),
                        note,
                        scope: *scope,
                        witness: IronwoodWitness::from_tree(self.tree.clone()).context("a witness of an empty tree")?,
                        spent_at: None,
                    });
                    break;
                }
            }
        }
        self.scanned_to = height;
        self.last_hash = block.hash.clone();
        Ok(())
    }

    /// The anchor (the Ironwood tree's root after `scanned_to`), for spends built now.
    pub fn anchor(&self) -> orchard::Anchor {
        orchard::Anchor::from(self.tree.root())
    }

    /// That the tree is the server's at `scanned_to`.
    pub async fn check_against_server(&self, client: &mut LwdClient) -> Result<()> {
        let server = lwd::tree_state(client, self.scanned_to).await?.ironwood_tree().context("the Ironwood tree")?;
        ensure!(server.root() == self.tree.root(), "the Ironwood tree is not the server's at {}", self.scanned_to);
        Ok(())
    }

    /// The unspent notes, in chain order.
    pub fn unspent(&self) -> Vec<&ReceivedNote> {
        let mut v: Vec<_> = self.notes.iter().filter(|n| n.spent_at.is_none()).collect();
        v.sort_by_key(|n| n.position);
        v
    }

    /// The sum of the unspent notes' values, in zatoshi.
    pub fn balance(&self) -> u64 {
        self.unspent().iter().map(|n| n.value()).sum()
    }
}

#[cfg(test)]
mod tests {
    use orchard::{
        keys::SpendingKey,
        note::{ExtractedNoteCommitment, NoteVersion, RandomSeed, Rho},
        note_encryption::IronwoodNoteEncryption,
        value::NoteValue,
    };
    use zcash_client_backend::proto::compact_formats::{CompactOrchardAction, CompactTx};
    use zcash_note_encryption::Domain;

    use super::*;

    pub(crate) fn test_fvk(sk: [u8; 32]) -> FullViewingKey {
        FullViewingKey::from(&SpendingKey::from_bytes(sk).unwrap())
    }

    fn block(height: u32, hash: u8, prev: u8) -> CompactBlock {
        CompactBlock {
            height: u64::from(height),
            hash: vec![hash; 32],
            prev_hash: vec![prev; 32],
            ..Default::default()
        }
    }

    /// A compact action that pays `value` to `fvk`'s external address, spending nullifier `nf`.
    fn paying_action(fvk: &FullViewingKey, value: u64, nf: [u8; 32]) -> CompactOrchardAction {
        let rho = Rho::from_bytes(&nf).unwrap();
        let rseed = (0u8..=255).find_map(|b| Option::from(RandomSeed::from_bytes([b; 32], &rho))).unwrap();
        let note = Note::from_parts(
            fvk.address_at(0u32, Scope::External),
            NoteValue::from_raw(value),
            rho,
            rseed,
            NoteVersion::V3,
        )
        .unwrap();
        let encryption = IronwoodNoteEncryption::new(None, note, [0; 512]);
        CompactOrchardAction {
            nullifier: nf.to_vec(),
            cmx: ExtractedNoteCommitment::from(note.commitment()).to_bytes().to_vec(),
            ephemeral_key: IronwoodDomain::epk_bytes(encryption.epk()).0.to_vec(),
            ciphertext: encryption.encrypt_note_plaintext().0[..52].to_vec(),
        }
    }

    #[test]
    fn scanning_follows_the_chain_and_refuses_a_reorganisation() {
        let mut s = ScanState::new(test_fvk([7; 32]), IronwoodTree::empty(), 99);
        s.scan_block(&block(100, 1, 0)).unwrap();
        s.scan_block(&block(101, 2, 1)).unwrap();
        // Block 102 does not build on the block 101 scanned.
        assert!(s.scan_block(&block(102, 3, 9)).unwrap_err().to_string().contains("reorganised"));
        assert_eq!(s.scanned_to, 101);
        assert!(s.scan_block(&block(103, 4, 2)).is_err(), "heights are consecutive");
        s.scan_block(&block(102, 3, 2)).unwrap();
        assert_eq!(s.scanned_to, 102);
        let mut top = ScanState::new(test_fvk([7; 32]), IronwoodTree::empty(), u32::MAX);
        assert!(top.scan_block(&block(u32::MAX, 1, 0)).is_err());
    }

    /// The wallet finds its note, with a witness whose path leads to the tree's root, and sees it spent by its
    /// nullifier; another key's notes and malformed actions change nothing.
    #[test]
    fn the_wallet_finds_its_notes_and_their_spends() {
        let fvk = test_fvk([7; 32]);
        let mut s = ScanState::new(fvk.clone(), IronwoodTree::empty(), 99);
        let mut b = block(100, 1, 0);
        b.vtx = vec![CompactTx {
            index: 3,
            ironwood_actions: vec![
                paying_action(&test_fvk([8; 32]), 5, [1; 32]),
                paying_action(&fvk, 70_000, [2; 32]),
            ],
            ..Default::default()
        }];
        s.scan_block(&b).unwrap();
        assert_eq!(s.notes.len(), 1);
        let n = &s.notes[0];
        assert_eq!((n.value(), n.scope, n.position.tx_index, n.position.action_index), (70_000, Scope::External, 3, 1));
        assert_eq!(s.balance(), 70_000);
        let cmx = ExtractedNoteCommitment::from(n.note.commitment());
        assert_eq!(n.merkle_path().unwrap().root(cmx), s.anchor());

        // A later block appends to the witness; then a spend of the note's nullifier.
        let mut later = block(101, 2, 1);
        later.vtx = vec![CompactTx {
            index: 0,
            ironwood_actions: vec![paying_action(&test_fvk([8; 32]), 9, [3; 32])],
            ..Default::default()
        }];
        s.scan_block(&later).unwrap();
        assert_eq!(s.notes[0].merkle_path().unwrap().root(cmx), s.anchor());
        let nf = s.notes[0].nullifier.to_bytes();
        let mut spend = block(102, 3, 2);
        spend.vtx = vec![CompactTx {
            index: 1,
            ironwood_actions: vec![paying_action(&test_fvk([8; 32]), 1, nf)],
            ..Default::default()
        }];
        s.scan_block(&spend).unwrap();
        assert_eq!(s.notes[0].spent_at, Some(102));
        assert_eq!((s.balance(), s.unspent().len()), (0, 0));

        let mut bad = block(103, 4, 3);
        bad.vtx = vec![CompactTx {
            index: 0,
            ironwood_actions: vec![CompactOrchardAction {
                cmx: vec![0xff; 32],
                ..paying_action(&fvk, 1, [4; 32])
            }],
            ..Default::default()
        }];
        let root = s.anchor();
        assert!(s.scan_block(&bad).is_err());
        assert_eq!((s.scanned_to, s.anchor()), (102, root), "nothing applied");
    }
}
