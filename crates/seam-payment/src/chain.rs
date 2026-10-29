//! The Zcash facts a payment proof rests on: the networks and their epochs, the Ironwood-only rule, and the inclusion
//! of a transaction in the chain history a later header commits to (ZIP 221, ZIP 244). Ported from Zecured's deposit
//! statement (`tier2/statement/src/lib.rs` at 73de9f909e270e8aea10c2b221905f62a6a4c4df), where they were checked on
//! real Testnet data.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tiny_keccak::{Hasher, Keccak};
use zcash_history::{NodeDataV3, V3, Version};
use zcash_primitives::transaction::{Authorized, TransactionData, TxVersion};

use crate::Error;

/// Consensus branch ID of NU6.3, the upgrade that added the Ironwood pool (ZIP 258).
pub const NU6_3_BRANCH_ID: u32 = 0x37A5_165B;
/// Consensus branch ID of NU7 (ZIP 259).
pub const NU7_BRANCH_ID: u32 = 0x7719_0AD9;
/// Serialized post-NU5 block header.
pub const HEADER_LENGTH: usize = 1487;

/// The Zcash network a build of the statement proves on. Fixed at build time ([`NETWORK`]): a witness never chooses
/// it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Network {
    /// `test` or `main`, as the guests' image ids are labelled (`payment-test`, `payment-main`).
    pub name: &'static str,
    /// The consensus branch ids of the epochs a proof may walk. A ZIP 221 history tree covers one epoch and restarts at
    /// every network upgrade, so B and T are always of the same epoch.
    pub branch_ids: &'static [u32],
}

impl Network {
    /// Whether a proof may walk the epoch whose consensus branch id is `branch_id`.
    pub fn allows(&self, branch_id: u32) -> bool {
        self.branch_ids.contains(&branch_id)
    }
}

/// Testnet: the NU7 epoch (activated at 4,465,026).
pub const TESTNET: Network = Network {
    name: "test",
    branch_ids: &[NU7_BRANCH_ID],
};

/// Mainnet: the Ironwood pool exists since NU6.3 (3,428,143); NU7 follows at 3,543,405 (ZIP 259).
pub const MAINNET: Network = Network {
    name: "main",
    branch_ids: &[NU6_3_BRANCH_ID, NU7_BRANCH_ID],
};

/// The network this build proves on: Testnet, or Mainnet with the cargo feature `mainnet`.
#[cfg(not(feature = "mainnet"))]
pub const NETWORK: Network = TESTNET;
/// The network this build proves on: Testnet, or Mainnet with the cargo feature `mainnet`.
#[cfg(feature = "mainnet")]
pub const NETWORK: Network = MAINNET;

/// One step of a history-tree path from the payment block's leaf up to its peak.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MmrStep {
    /// The sibling node, serialized (ZIP 221 V3).
    pub sibling: Vec<u8>,
    /// Whether the sibling is the left child.
    pub sibling_is_left: bool,
}

pub fn sha256d(data: &[u8]) -> [u8; 32] {
    Sha256::digest(Sha256::digest(data)).into()
}

pub fn keccak256(data: &[u8]) -> [u8; 32] {
    let mut k = Keccak::v256();
    k.update(data);
    let mut out = [0u8; 32];
    k.finalize(&mut out);
    out
}

fn blake2b_256(personal: &[u8; 16], parts: &[&[u8]]) -> [u8; 32] {
    let mut state = blake2b_simd::Params::new().hash_length(32).personal(personal).to_state();
    for p in parts {
        state.update(p);
    }
    state.finalize().as_bytes().try_into().expect("32 bytes")
}

/// The root of a SHA-256d Merkle path (Bitcoin-style tree, as Zcash's `hashMerkleRoot`).
pub fn txid_path_root(txid: [u8; 32], index: u32, path: &[[u8; 32]]) -> [u8; 32] {
    let mut node = txid;
    let mut i = index;
    for sibling in path {
        node = if i & 1 == 0 {
            sha256d(&[node, *sibling].concat())
        } else {
            sha256d(&[*sibling, node].concat())
        };
        i >>= 1;
    }
    node
}

/// `hashBlockCommitments` (ZIP 244).
pub fn block_commitments(history_root: &[u8; 32], auth_data_root: &[u8; 32]) -> [u8; 32] {
    blake2b_256(b"ZcashBlockCommit", &[history_root, auth_data_root, &[0u8; 32]])
}

/// The Ironwood-only rule on network `net`: a v6 transaction of an allowed epoch whose only bundle is an Ironwood one
/// with outputs enabled; no transparent, Sapling or Orchard-pool bundle, even an empty one. (Zecured's deposit rule
/// D5, `check_deposit_transaction_on`.)
pub fn check_payment_transaction_on(net: &Network, tx: &TransactionData<Authorized>) -> Result<(), Error> {
    if !matches!(tx.version(), TxVersion::V6) {
        return Err(Error::NotV6);
    }
    if !net.allows(u32::from(tx.consensus_branch_id())) {
        return Err(Error::WrongEpoch);
    }
    if tx.transparent_bundle().is_some() || tx.sapling_bundle().is_some() || tx.orchard_bundle().is_some() {
        return Err(Error::NotIronwoodOnly);
    }
    let bundle = tx.ironwood_bundle().ok_or(Error::NotIronwoodOnly)?;
    if !bundle.flags().outputs_enabled() {
        return Err(Error::OutputsDisabled);
    }
    Ok(())
}

fn node(branch_id: u32, bytes: &[u8]) -> Result<NodeDataV3, Error> {
    <V3 as Version>::from_bytes(branch_id, bytes).map_err(|_| Error::HistoryPath)
}

/// Where a transaction is: its block B and B's place in the chain history a later header T commits to.
#[derive(Clone, Copy, Debug)]
pub struct Inclusion<'a> {
    /// Consensus branch ID of the epoch of B and T. It personalizes every node hash of the history tree (ZIP 221), so
    /// B's leaf reaches T's commitment only under T's own epoch: B and T are of the same epoch. The caller checks the
    /// network allows it.
    pub branch_id: u32,
    /// The transaction's position in block B and the SHA-256d Merkle path from its txid.
    pub tx_index: u32,
    pub txid_path: &'a [[u8; 32]],
    /// Block B's header.
    pub block_header: &'a [u8],
    /// B's history-tree leaf (ZIP 221 V3), its path to a peak, all peaks (oldest first) and which peak the path
    /// reaches.
    pub leaf: &'a [u8],
    pub mmr_path: &'a [MmrStep],
    pub peaks: &'a [Vec<u8>],
    pub peak_index: u32,
    /// Header T and its block's auth data root (ZIP 244).
    pub tip_header: &'a [u8],
    pub tip_auth_data_root: &'a [u8; 32],
}

/// What [`check_inclusion`] establishes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Included {
    /// Block B's height (hidden: only T is public).
    pub block_height: u64,
    /// SHA-256d of header T, internal byte order (as the relay stores it).
    pub tip_hash: [u8; 32],
}

/// Proves that `txid` is in a block B that is a leaf of the chain history of epoch `i.branch_id` committed to by
/// header T: the txid's Merkle path reaches B's `merkleRoot`, B's ZIP 221 leaf (B alone) is on a path to one of the
/// peaks, and the peaks with T's auth data root hash to T's `hashBlockCommitments`.
pub fn check_inclusion(txid: &[u8; 32], i: &Inclusion<'_>) -> Result<Included, Error> {
    if i.block_header.len() != HEADER_LENGTH || i.tip_header.len() != HEADER_LENGTH {
        return Err(Error::Header);
    }
    let merkle_root: [u8; 32] = i.block_header[36..68].try_into().expect("32 bytes");
    if txid_path_root(*txid, i.tx_index, i.txid_path) != merkle_root {
        return Err(Error::TxidNotInBlock);
    }

    let leaf = node(i.branch_id, i.leaf)?;
    let v1 = &leaf.v2.v1;
    if v1.subtree_commitment != sha256d(i.block_header) || v1.start_height != v1.end_height {
        return Err(Error::LeafIsNotTheBlock);
    }
    let block_height = v1.end_height;
    let mut acc = leaf;
    for step in i.mmr_path {
        let sibling = node(i.branch_id, &step.sibling)?;
        acc = if step.sibling_is_left {
            V3::combine(&sibling, &acc)
        } else {
            V3::combine(&acc, &sibling)
        };
    }
    let peak = i.peaks.get(i.peak_index as usize).ok_or(Error::HistoryPath)?;
    if V3::to_bytes(&acc) != *peak {
        return Err(Error::HistoryPath);
    }
    let mut peaks = i.peaks.iter();
    let mut root = node(i.branch_id, peaks.next().ok_or(Error::HistoryPath)?)?;
    for p in peaks {
        root = V3::combine(&root, &node(i.branch_id, p)?);
    }
    let tip_commitments: [u8; 32] = i.tip_header[68..100].try_into().expect("32 bytes");
    if block_commitments(&V3::hash(&root), i.tip_auth_data_root) != tip_commitments {
        return Err(Error::HistoryNotCommitted);
    }
    Ok(Included {
        block_height,
        tip_hash: sha256d(i.tip_header),
    })
}
