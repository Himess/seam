//! K4 for the payment statement: the synthetic payment fixture and the single changes to a payment witness, each with
//! the outcome the statement must give. The crate's tests run them natively; `seam parity` runs them in the RISC Zero
//! executor, where the guest must panic with the same error or commit the same journal. Behind the `k4` feature: the
//! guest never compiles it. Adapted from Zecured's `tier2/payment-k4` (73de9f909e270e8aea10c2b221905f62a6a4c4df).
//!
//! **The fixture is synthetic** (TEST-ONLY) and built from public chain data only: [`PublicTx`] is a real
//! Ironwood-only v6 transaction mined on Zcash Testnet with its real inclusion data. [`synthesize`] re-encrypts one of
//! its actions as a payment note to a test receiver, with the memo of [`fixture_payment_ref`], and re-anchors the
//! changed transaction ([`reanchor`]): its block B' is B with the Merkle root the changed txid reaches along B's real
//! path, its history-tree leaf is B''s, the real siblings lead to a new peak, and header T' is T with the
//! `hashBlockCommitments` of the new peaks. The statement checks no proof of work and no Halo 2 proof or signature
//! (the relay and Zcash consensus do), so it accepts this witness; a relay would never accept T' (its hash is in no
//! chain). Only the payment note is new: the transaction's shape, its epoch, B's height and position, the history
//! tree's shape and every other byte are the real ones.

use std::fmt;

use orchard::{
    Address, Note,
    keys::{FullViewingKey, Scope, SpendingKey},
    note::{ExtractedNoteCommitment, NoteVersion, RandomSeed, Rho},
    note_encryption::{ENC_CIPHERTEXT_SIZE, IronwoodDomain, IronwoodNoteEncryption},
    value::NoteValue,
};
use serde::{Deserialize, Serialize};
use zcash_history::{NodeDataV3, V2, V3, Version};
use zcash_note_encryption::{Domain, OUT_CIPHERTEXT_SIZE};
use zcash_primitives::transaction::Transaction;
use zcash_protocol::{
    consensus::BranchId,
    constants::{V5_TX_VERSION, V5_VERSION_GROUP_ID},
};

use crate::{
    Error, MEMO_BINDING_LENGTH, MEMO_LENGTH, MmrStep, NU6_3_BRANCH_ID, Network, PaymentJournal, PaymentWitness,
    block_commitments, keccak256, payment_memo, receiver_commitment, sha256d, txid_path_root,
};

/// The fixture's reference: `paymentRef = keccak256(abi.encode(uint256 chainId, address escrow, uint256 orderId))`,
/// as `ShieldedPaymentEscrow` derives it, with these values (its tests check the same vector).
pub const FIXTURE_CHAIN_ID: u64 = 84_532;
pub const FIXTURE_ESCROW: [u8; 20] = [0x11; 20];
pub const FIXTURE_ORDER_ID: u64 = 7;
/// The salt of the fixture's receiver commitment.
pub const FIXTURE_SALT: [u8; 32] = [0x5a; 32];
/// The payment note's value (0.025 TAZ) and rseed.
pub const FIXTURE_VALUE: u64 = 2_500_000;
pub const FIXTURE_RSEED: [u8; 32] = [0x42; 32];
/// The spending keys of the fixture's receiver and of another valid receiver: public test values, which hold no funds.
/// Their addresses are valid on Mainnet too, and anyone can spend what is sent to them: never pay them.
pub const FIXTURE_RECEIVER_SPENDING_KEY: [u8; 32] = [0x07; 32];
pub const OTHER_RECEIVER_SPENDING_KEY: [u8; 32] = [0x08; 32];

/// A real Ironwood-only v6 transaction and its real inclusion data: public chain data, as any node serves it. It has
/// no note opening: [`synthesize`] makes the payment note.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PublicTx {
    pub branch_id: u32,
    pub tx: Vec<u8>,
    /// The action [`synthesize`] re-encrypts.
    pub action_index: u32,
    pub tx_index: u32,
    pub txid_path: Vec<[u8; 32]>,
    pub block_header: Vec<u8>,
    pub leaf: Vec<u8>,
    pub mmr_path: Vec<MmrStep>,
    pub peaks: Vec<Vec<u8>>,
    pub peak_index: u32,
    pub tip_header: Vec<u8>,
    pub tip_auth_data_root: [u8; 32],
}

/// `keccak256(abi.encode(uint256 chainId, address escrow, uint256 orderId))`.
pub fn payment_ref(chain_id: u64, escrow: &[u8; 20], order_id: u64) -> [u8; 32] {
    let mut enc = [0u8; 3 * 32];
    enc[24..32].copy_from_slice(&chain_id.to_be_bytes());
    enc[32 + 12..64].copy_from_slice(escrow);
    enc[64 + 24..96].copy_from_slice(&order_id.to_be_bytes());
    keccak256(&enc)
}

/// The fixture's reference.
pub fn fixture_payment_ref() -> [u8; 32] {
    payment_ref(FIXTURE_CHAIN_ID, &FIXTURE_ESCROW, FIXTURE_ORDER_ID)
}

/// The external address at diversifier index `index` of the test spending key `sk`, as a 43-byte raw receiver.
pub fn test_receiver_at(sk: [u8; 32], index: u32) -> [u8; 43] {
    let sk = SpendingKey::from_bytes(sk).expect("a valid spending key");
    FullViewingKey::from(&sk).address_at(index, Scope::External).to_raw_address_bytes()
}

/// The external address at diversifier index 0 of the test spending key `sk`, as a 43-byte raw receiver.
pub fn test_receiver(sk: [u8; 32]) -> [u8; 43] {
    test_receiver_at(sk, 0)
}

/// The fixture's receiver.
pub fn fixture_receiver() -> [u8; 43] {
    test_receiver(FIXTURE_RECEIVER_SPENDING_KEY)
}

/// What a journal binds a payment to: its reference, receiver commitment and amount.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binding {
    pub payment_ref: [u8; 32],
    pub receiver_commitment: [u8; 32],
    pub zatoshi: u64,
}

impl Binding {
    /// What journal `j` binds.
    pub fn of(j: &PaymentJournal) -> Self {
        Binding {
            payment_ref: j.payment_ref,
            receiver_commitment: j.receiver_commitment,
            zatoshi: j.zatoshi,
        }
    }

    /// What the synthetic fixture binds: [`fixture_payment_ref`], [`fixture_receiver`] under [`FIXTURE_SALT`] and
    /// [`FIXTURE_VALUE`].
    pub fn fixture() -> Self {
        Binding {
            payment_ref: fixture_payment_ref(),
            receiver_commitment: receiver_commitment(&FIXTURE_SALT, &fixture_receiver()),
            zatoshi: FIXTURE_VALUE,
        }
    }
}

/// What a change must lead to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expected {
    /// The statement refuses the witness with this error (the guest panics with it).
    Refused(Error),
    /// The statement accepts the witness with the base journal's binding: the change is one the statement does not
    /// see, or a re-anchoring.
    Accepted,
    /// The statement accepts the witness with exactly this binding, not the base's: an app refuses it for the base's
    /// payment.
    Rebound(Binding),
}

/// The outcome alone, without a rebinding's values: `Refused(Memo)`, `Accepted`, `Rebound`.
impl fmt::Display for Expected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expected::Refused(e) => write!(f, "Refused({e:?})"),
            Expected::Accepted => f.write_str("Accepted"),
            Expected::Rebound(_) => f.write_str("Rebound"),
        }
    }
}

/// Whether `native`, the statement's outcome on a changed witness, is what `expected` asks, given the base witness's
/// journal `base`.
pub fn as_expected(expected: Expected, base: &PaymentJournal, native: &Result<PaymentJournal, Error>) -> bool {
    let same_epoch = |j: &PaymentJournal| j.version == base.version && j.branch_id == base.branch_id;
    match (expected, native) {
        (Expected::Refused(e), Err(got)) => *got == e,
        (Expected::Accepted, Ok(j)) => same_epoch(j) && Binding::of(j) == Binding::of(base),
        (Expected::Rebound(b), Ok(j)) => same_epoch(j) && Binding::of(j) == b && b != Binding::of(base),
        _ => false,
    }
}

/// One change and the outcome it must have.
pub struct Mutation {
    pub name: &'static str,
    pub apply: fn(&mut PaymentWitness),
    pub expected: Expected,
}

/// `p` as a payment witness with an opening: the real transaction and anchor, unchanged.
pub fn with_opening(p: &PublicTx, recipient: [u8; 43], value: u64, rseed: [u8; 32], salt: [u8; 32]) -> PaymentWitness {
    PaymentWitness {
        branch_id: p.branch_id,
        tx: p.tx.clone(),
        action_index: p.action_index,
        note_recipient: recipient.to_vec(),
        note_value: value,
        note_rseed: rseed,
        salt,
        tx_index: p.tx_index,
        txid_path: p.txid_path.clone(),
        block_header: p.block_header.clone(),
        leaf: p.leaf.clone(),
        mmr_path: p.mmr_path.clone(),
        peaks: p.peaks.clone(),
        peak_index: p.peak_index,
        tip_header: p.tip_header.clone(),
        tip_auth_data_root: p.tip_auth_data_root,
    }
}

/// The synthetic fixture from the public transaction `p`: its action re-encrypted as a payment of [`FIXTURE_VALUE`]
/// to [`fixture_receiver`], with the memo of [`fixture_payment_ref`], under the salt [`FIXTURE_SALT`], re-anchored
/// ([`reanchor`]).
pub fn synthesize(p: &PublicTx) -> PaymentWitness {
    let mut w = with_opening(p, fixture_receiver(), FIXTURE_VALUE, FIXTURE_RSEED, FIXTURE_SALT);
    reencrypt(&mut w, &payment_memo(&fixture_payment_ref()));
    reanchor(&mut w);
    w
}

/// Re-encrypts the witness's action as the note the witness opens (its receiver, value and rseed, rho the action's
/// nullifier), with `memo` and `ovk = None`, in place in the serialized transaction: the action's note commitment,
/// ephemeral key and note ciphertext change; its value commitment, key, proof and signatures do not (the statement
/// checks none of them). The txid changes: [`reanchor`] puts the transaction back under an anchor.
pub fn reencrypt(w: &mut PaymentWitness, memo: &[u8; MEMO_LENGTH]) {
    reencrypt_as(w, memo, NoteVersion::V3);
}

/// [`reencrypt`], with a note of `version`: the Ironwood pool's notes are V3 (ZIP 2005).
fn reencrypt_as(w: &mut PaymentWitness, memo: &[u8; MEMO_LENGTH], version: NoteVersion) {
    let tx = Transaction::read(&w.tx[..], BranchId::Nu7).expect("a v6 transaction");
    let bundle = tx.ironwood_bundle().expect("an Ironwood bundle");
    let action = &bundle.actions()[w.action_index as usize];
    // v6 serializes an action as cv_net || nf || rk || cmx || epk || enc_ciphertext || out_ciphertext
    // (zcash_primitives' write_action_without_auth): cmx and epk are adjacent and unique.
    let old: Vec<u8> = [action.cmx().to_bytes(), action.encrypted_note().epk_bytes].concat();
    let rho = Rho::from_bytes(&action.nullifier().to_bytes()).expect("the action's nullifier");
    let at = {
        let mut found = w.tx.windows(old.len()).enumerate().filter(|(_, s)| *s == old.as_slice()).map(|(i, _)| i);
        let at = found.next().expect("the action's cmx and epk in the transaction");
        assert!(found.next().is_none(), "the action's cmx and epk occur once");
        at
    };

    let recipient: [u8; 43] = w.note_recipient.clone().try_into().expect("a 43-byte receiver");
    let address = Address::from_raw_address_bytes(&recipient).expect("a valid receiver");
    let rseed = RandomSeed::from_bytes(w.note_rseed, &rho).expect("an rseed valid for the action's rho");
    let note = Note::from_parts(address, NoteValue::from_raw(w.note_value), rho, rseed, version).expect("a valid note");
    let cmx = ExtractedNoteCommitment::from(note.commitment()).to_bytes();
    let encryption = IronwoodNoteEncryption::new(None, note, *memo);
    let epk = IronwoodDomain::epk_bytes(encryption.epk()).0;
    let enc = encryption.encrypt_note_plaintext().0;

    w.tx[at..at + 32].copy_from_slice(&cmx);
    w.tx[at + 32..at + 64].copy_from_slice(&epk);
    w.tx[at + 64..at + 64 + enc.len()].copy_from_slice(&enc);
    let changed = Transaction::read(&w.tx[..], BranchId::Nu7).expect("still a v6 transaction");
    let action = &changed.ironwood_bundle().expect("an Ironwood bundle").actions()[w.action_index as usize];
    assert_eq!(action.cmx().to_bytes(), cmx);
    assert_eq!(action.encrypted_note().epk_bytes, epk);
    assert_eq!(action.encrypted_note().enc_ciphertext.0, enc);
}

fn node(branch_id: u32, bytes: &[u8]) -> NodeDataV3 {
    V3::from_bytes(branch_id, bytes).expect("a V3 history node")
}

/// Puts the witness's transaction, as it now is, back under an anchor, keeping the shape of its inclusion: block B' is
/// B with the Merkle root the txid reaches along the witness's txid path and index; B''s history-tree leaf replaces
/// B's; the witness's real siblings lead from it to a new peak at the same place; header T''s `hashBlockCommitments`
/// commits to the new peaks (under the witness's epoch, `w.branch_id`). Every inclusion check passes again; only
/// header hashes change. The statement checks no proof of work, so it accepts this anchor; a relay never would.
pub fn reanchor(w: &mut PaymentWitness) {
    let tx = Transaction::read(&w.tx[..], BranchId::Nu7).expect("a v6 transaction");
    let txid: [u8; 32] = *tx.txid().as_ref();
    let root = txid_path_root(txid, w.tx_index, &w.txid_path);
    w.block_header[36..68].copy_from_slice(&root);
    let mut leaf = node(w.branch_id, &w.leaf);
    leaf.v2.v1.subtree_commitment = sha256d(&w.block_header);
    w.leaf = V3::to_bytes(&leaf);
    let mut acc = leaf;
    for step in &w.mmr_path {
        let sibling = node(w.branch_id, &step.sibling);
        acc = if step.sibling_is_left {
            V3::combine(&sibling, &acc)
        } else {
            V3::combine(&acc, &sibling)
        };
    }
    w.peaks[w.peak_index as usize] = V3::to_bytes(&acc);
    let mut peaks = w.peaks.iter();
    let mut history = node(w.branch_id, peaks.next().expect("a peak"));
    for p in peaks {
        history = V3::combine(&history, &node(w.branch_id, p));
    }
    let commitments = block_commitments(&V3::hash(&history), &w.tip_auth_data_root);
    w.tip_header[68..100].copy_from_slice(&commitments);
}

fn with_leaf(w: &mut PaymentWitness, change: impl FnOnce(&mut NodeDataV3)) {
    let mut leaf = node(w.branch_id, &w.leaf);
    change(&mut leaf);
    w.leaf = V3::to_bytes(&leaf);
}

/// The witness's transaction with one transparent output added (the Ironwood-only rule forbids it).
pub fn add_transparent_output(w: &mut PaymentWitness) {
    // v6: 20 header bytes, then the transparent input and output counts (both 0).
    assert_eq!(&w.tx[20..22], &[0, 0]);
    let mut tx = w.tx[..21].to_vec();
    tx.push(1);
    tx.extend_from_slice(&1_000u64.to_le_bytes());
    tx.push(0);
    tx.extend_from_slice(&w.tx[22..]);
    w.tx = tx;
}

/// The witness's transaction with one transparent input added (the Ironwood-only rule forbids it).
pub fn add_transparent_input(w: &mut PaymentWitness) {
    // v6: 20 header bytes, then the transparent input and output counts (both 0).
    assert_eq!(&w.tx[20..22], &[0, 0]);
    let mut tx = w.tx[..20].to_vec();
    tx.push(1);
    tx.extend_from_slice(&[0x11; 32]); // the outpoint's txid
    tx.extend_from_slice(&0u32.to_le_bytes()); // and output index
    tx.push(0); // an empty script
    tx.extend_from_slice(&u32::MAX.to_le_bytes()); // the sequence
    tx.extend_from_slice(&w.tx[21..]);
    w.tx = tx;
}

/// Length of a serialized Ironwood action without its signature: cv_net, nf, rk, cmx, epk and both ciphertexts.
const ACTION_LENGTH: usize = 5 * 32 + ENC_CIPHERTEXT_SIZE + OUT_CIPHERTEXT_SIZE;

/// The witness's transaction with the 32 bytes at `offset` in its first action all 0xff: above the field's modulus,
/// so no canonical encoding of a field element or a point (an action is cv_net, nf, rk, cmx, epk, ... in that order).
fn non_canonical(w: &mut PaymentWitness, offset: usize) {
    // v6: 20 header bytes, five empty counts, the Ironwood action count, then its first action.
    assert_eq!(&w.tx[20..25], &[0, 0, 0, 0, 0]);
    let at = 26 + offset;
    w.tx[at..at + 32].fill(0xff);
}

/// The witness's transaction with a copy of its Ironwood bundle in the Orchard pool's place (the Ironwood-only rule
/// forbids any Orchard-pool bundle). The copy's flags clear `enableCrossAddress`, which an Orchard-pool bundle may not
/// set: the transaction still parses.
pub fn add_orchard_pool_bundle(w: &mut PaymentWitness) {
    // v6: 20 header bytes; no transparent inputs or outputs, no Sapling spends or outputs; an empty Orchard-pool bundle
    // (one zero byte); then the Ironwood bundle, which ends the transaction: its action count, its actions, its flags.
    assert_eq!(&w.tx[20..25], &[0, 0, 0, 0, 0]);
    let ironwood = w.tx[25..].to_vec();
    let actions = usize::from(ironwood[0]);
    assert!(actions < 0xfd, "a one-byte action count");
    let mut orchard = ironwood.clone();
    orchard[1 + actions * ACTION_LENGTH] &= !0b100;
    w.tx.truncate(24);
    w.tx.extend_from_slice(&orchard);
    w.tx.extend_from_slice(&ironwood);
}

/// The witness's transaction with its own consensus branch id set to `branch_id`.
pub fn set_transaction_branch_id(w: &mut PaymentWitness, branch_id: u32) {
    // v6: the header word, the version group id, then the consensus branch id (little-endian).
    assert_eq!(BranchId::try_from(u32::from_le_bytes(w.tx[8..12].try_into().expect("4 bytes"))), Ok(BranchId::Nu7));
    w.tx[8..12].copy_from_slice(&branch_id.to_le_bytes());
}

/// The witness's transaction without its Ironwood bundle.
fn drop_ironwood_bundle(w: &mut PaymentWitness) {
    // v6: 20 header bytes; no transparent inputs or outputs, no Sapling spends or outputs, no Orchard-pool actions;
    // then the Ironwood bundle (an empty bundle is one zero byte).
    assert_eq!(&w.tx[20..25], &[0, 0, 0, 0, 0]);
    w.tx.truncate(25);
    w.tx.push(0);
}

/// The witness's transaction with its Ironwood bundle's `enableOutputs` flag cleared.
fn disable_outputs(w: &mut PaymentWitness) {
    // v6: 20 header bytes, five empty counts, then the Ironwood bundle: its action count, its actions, its flags.
    assert_eq!(&w.tx[20..25], &[0, 0, 0, 0, 0]);
    let actions = usize::from(w.tx[25]);
    assert!(actions < 0xfd, "a one-byte action count");
    w.tx[25 + 1 + actions * ACTION_LENGTH] &= !0b10;
}

/// The witness's transaction with a v5 transaction's header. It still parses: as a v5 transaction with no bundles,
/// whose format ends where the Ironwood bundle starts.
fn with_v5_header(w: &mut PaymentWitness) {
    // The header word (overwintered | version), then the version group id, both little-endian.
    w.tx[0..4].copy_from_slice(&(V5_TX_VERSION | 1 << 31).to_le_bytes());
    w.tx[4..8].copy_from_slice(&V5_VERSION_GROUP_ID.to_le_bytes());
}

/// Re-encrypts the witness's note with `memo` and re-anchors it: the memo rule decides alone.
fn with_memo(w: &mut PaymentWitness, memo: [u8; MEMO_LENGTH]) {
    reencrypt(w, &memo);
    reanchor(w);
}

/// The catalog for the statement built for `net`, in the statement's order of checks. The base is the synthetic
/// fixture ([`synthesize`]): the memo changes write its reference, and each rebinding is exactly its binding with one
/// field changed.
pub fn mutations(net: &Network) -> Vec<Mutation> {
    use Error::*;
    use Expected::*;
    let m = |name, apply: fn(&mut PaymentWitness), expected| Mutation { name, apply, expected };
    let fixture = Binding::fixture();
    let mut other_salt = FIXTURE_SALT;
    other_salt[0] ^= 1;
    let mut catalog = vec![
        // 1. The epoch: the build allows it, the witness only names it.
        m("a branch id of no epoch", |w| w.branch_id = 0xdead_beef, Refused(BranchNotAllowed)),
        m(
            "the branch id of NU6.2 (no Ironwood pool)",
            |w| w.branch_id = 0x5437_f330,
            Refused(BranchNotAllowed),
        ),
    ];
    if net.allows(NU6_3_BRANCH_ID) {
        catalog.extend([
            // An allowed epoch, but not the one of B and T: the history does not open under NU6.3's personalization.
            m(
                "B and T in another epoch than the witness's (NU6.3)",
                |w| w.branch_id = NU6_3_BRANCH_ID,
                Refused(HistoryPath),
            ),
            // A history that does open under NU6.3, with the transaction of another epoch.
            m(
                "B and T re-anchored in NU6.3's history, the transaction NU7's",
                |w| {
                    w.branch_id = NU6_3_BRANCH_ID;
                    reanchor(w);
                },
                Refused(WrongEpoch),
            ),
        ]);
    } else {
        catalog.push(m(
            "the branch id of NU6.3 (not an epoch this build proves)",
            |w| w.branch_id = NU6_3_BRANCH_ID,
            Refused(BranchNotAllowed),
        ));
    }
    catalog.extend([
        // 2. The transaction: Ironwood only.
        m("truncated transaction", |w| w.tx.truncate(100), Refused(Transaction)),
        m("a v5 transaction's header", with_v5_header, Refused(NotV6)),
        m("a transparent output", add_transparent_output, Refused(NotIronwoodOnly)),
        m("a transparent input", add_transparent_input, Refused(NotIronwoodOnly)),
        m("no Ironwood bundle", drop_ironwood_bundle, Refused(NotIronwoodOnly)),
        m("a nullifier that is no field element", |w| non_canonical(w, 32), Refused(Transaction)),
        m("a note commitment that is no field element", |w| non_canonical(w, 96), Refused(Transaction)),
        m("an ephemeral key that is no point", |w| non_canonical(w, 128), Refused(Transaction)),
        m("the Ironwood bundle's outputs disabled", disable_outputs, Refused(OutputsDisabled)),
        m("action index out of range", |w| w.action_index = 99, Refused(NoSuchAction)),
        m("re-anchored, unchanged (the statement checks no proof of work)", reanchor, Accepted),
        m(
            "a transparent output, re-anchored",
            |w| {
                add_transparent_output(w);
                reanchor(w);
            },
            Refused(NotIronwoodOnly),
        ),
        m(
            "a transparent input, re-anchored",
            |w| {
                add_transparent_input(w);
                reanchor(w);
            },
            Refused(NotIronwoodOnly),
        ),
        m(
            "an Orchard-pool copy of the Ironwood bundle, re-anchored",
            |w| {
                add_orchard_pool_bundle(w);
                reanchor(w);
            },
            Refused(NotIronwoodOnly),
        ),
        m(
            "the transaction's own branch id NU6.3's, re-anchored",
            |w| {
                set_transaction_branch_id(w, NU6_3_BRANCH_ID);
                reanchor(w);
            },
            Refused(WrongEpoch),
        ),
        // 3. Block B in T's chain history (ZIP 221).
        m(
            "block header one byte short",
            |w| {
                w.block_header.pop();
            },
            Refused(Header),
        ),
        m("tip header one byte long", |w| w.tip_header.push(0), Refused(Header)),
        m("txid path changed", |w| w.txid_path[0][0] ^= 1, Refused(TxidNotInBlock)),
        m("transaction index changed", |w| w.tx_index ^= 1, Refused(TxidNotInBlock)),
        m(
            "txid path one step short",
            |w| {
                w.txid_path.pop();
            },
            Refused(TxidNotInBlock),
        ),
        m("another Merkle root in B", |w| w.block_header[40] ^= 1, Refused(TxidNotInBlock)),
        m("leaf spans two blocks", |w| with_leaf(w, |l| l.v2.v1.start_height -= 1), Refused(LeafIsNotTheBlock)),
        m(
            "leaf of another block",
            |w| with_leaf(w, |l| l.v2.v1.subtree_commitment = [0; 32]),
            Refused(LeafIsNotTheBlock),
        ),
        m("another block B", |w| w.block_header[1000] ^= 1, Refused(LeafIsNotTheBlock)),
        m(
            "leaf of another epoch (V2 node)",
            |w| {
                let leaf = node(w.branch_id, &w.leaf);
                w.leaf = V2::to_bytes(&leaf.v2);
            },
            Refused(HistoryPath),
        ),
        m("history path on the wrong side", |w| w.mmr_path[0].sibling_is_left ^= true, Refused(HistoryPath)),
        m("history path sibling changed", |w| w.mmr_path[0].sibling[0] ^= 1, Refused(HistoryPath)),
        m(
            "history path one step short",
            |w| {
                w.mmr_path.pop();
            },
            Refused(HistoryPath),
        ),
        m("peak index out of range", |w| w.peak_index = 99, Refused(HistoryPath)),
        m("another peak", |w| w.peak_index -= 1, Refused(HistoryPath)),
        m("an earlier peak changed", |w| w.peaks[0][0] ^= 1, Refused(HistoryNotCommitted)),
        m(
            "a peak missing",
            |w| {
                w.peaks.remove(0);
                w.peak_index -= 1;
            },
            Refused(HistoryNotCommitted),
        ),
        m("another auth data root", |w| w.tip_auth_data_root[0] ^= 1, Refused(HistoryNotCommitted)),
        m("T's hashBlockCommitments changed", |w| w.tip_header[80] ^= 1, Refused(HistoryNotCommitted)),
        // The header's nonce (bytes 108..140) is outside what the statement checks: the anchor, T's hash, moves.
        m("T's nonce changed", |w| w.tip_header[108] ^= 1, Accepted),
        // 4. The note.
        m("the other action of the transaction", |w| w.action_index ^= 1, Refused(NotDecryptable)),
        m("value + 1", |w| w.note_value += 1, Refused(NoteCommitmentMismatch)),
        m("value - 1", |w| w.note_value -= 1, Refused(NoteCommitmentMismatch)),
        m("another rseed", |w| w.note_rseed[0] ^= 1, Refused(NotDecryptable)),
        // The Ironwood domain parses V3 plaintexts only: Error::NoteVersion is unreachable.
        m(
            "a V2 note (ZIP 212) in the action, re-anchored",
            |w| {
                reencrypt_as(w, &payment_memo(&fixture_payment_ref()), orchard::note::NoteVersion::V2);
                reanchor(w);
            },
            Refused(NotDecryptable),
        ),
        m("receiver not 43 bytes", |w| w.note_recipient.truncate(42), Refused(Note)),
        m("pk_d not a point", |w| w.note_recipient[11..].fill(0xff), Refused(Note)),
        m("pk_d the identity", |w| w.note_recipient[11..].fill(0), Refused(Note)),
        // esk derives from rseed and rho alone (orchard 0.16): under the opening's esk and pk_d the ciphertext still
        // opens, to the note with the real diversifier.
        m("the diversifier changed, pk_d kept", |w| w.note_recipient[0] ^= 1, Refused(NoteCommitmentMismatch)),
        m(
            "another address of the receiver's wallet (diversifier index 1)",
            |w| w.note_recipient = test_receiver_at(FIXTURE_RECEIVER_SPENDING_KEY, 1).to_vec(),
            Refused(NotDecryptable),
        ),
        m(
            "another receiver (a valid one)",
            |w| w.note_recipient = test_receiver(OTHER_RECEIVER_SPENDING_KEY).to_vec(),
            Refused(NotDecryptable),
        ),
        // 5. The memo binds one reference (re-encrypted and re-anchored: the rule decides alone).
        m(
            "a memo with another prefix (seam2:)",
            |w| {
                let mut memo = payment_memo(&fixture_payment_ref());
                memo[4] = b'2';
                with_memo(w, memo);
            },
            Refused(Memo),
        ),
        m(
            "the reference's digits in uppercase",
            |w| {
                let mut memo = payment_memo(&fixture_payment_ref());
                memo[6..MEMO_BINDING_LENGTH].make_ascii_uppercase();
                with_memo(w, memo);
            },
            Refused(Memo),
        ),
        m(
            "the reference's digits without the prefix",
            |w| {
                let mut memo = [0u8; MEMO_LENGTH];
                memo[..64].copy_from_slice(&payment_memo(&fixture_payment_ref())[6..MEMO_BINDING_LENGTH]);
                with_memo(w, memo);
            },
            Refused(Memo),
        ),
        m(
            "the reference as 32 raw bytes after the prefix",
            |w| {
                let mut memo = [0u8; MEMO_LENGTH];
                memo[..6].copy_from_slice(b"seam1:");
                memo[6..38].copy_from_slice(&fixture_payment_ref());
                with_memo(w, memo);
            },
            Refused(Memo),
        ),
        m(
            "the empty memo (ZIP 302: 0xF6)",
            |w| {
                let mut memo = [0u8; MEMO_LENGTH];
                memo[0] = 0xf6;
                with_memo(w, memo);
            },
            Refused(Memo),
        ),
        m(
            "a payment for another reference",
            |w| with_memo(w, payment_memo(&payment_ref(FIXTURE_CHAIN_ID, &FIXTURE_ESCROW, FIXTURE_ORDER_ID + 1))),
            Rebound(Binding {
                payment_ref: payment_ref(FIXTURE_CHAIN_ID, &FIXTURE_ESCROW, FIXTURE_ORDER_ID + 1),
                ..fixture
            }),
        ),
        m(
            "the same reference, text after the binding",
            |w| {
                let mut memo = payment_memo(&fixture_payment_ref());
                memo[MEMO_BINDING_LENGTH..MEMO_BINDING_LENGTH + 7].copy_from_slice(b" thanks");
                with_memo(w, memo);
            },
            Accepted,
        ),
        // 6. The receiver commitment and the amount are the journal's to bind.
        m(
            "a payment to another receiver, with its opening",
            |w| {
                w.note_recipient = test_receiver(OTHER_RECEIVER_SPENDING_KEY).to_vec();
                with_memo(w, payment_memo(&fixture_payment_ref()));
            },
            Rebound(Binding {
                receiver_commitment: receiver_commitment(&FIXTURE_SALT, &test_receiver(OTHER_RECEIVER_SPENDING_KEY)),
                ..fixture
            }),
        ),
        m(
            "another salt",
            |w| w.salt[0] ^= 1,
            Rebound(Binding {
                receiver_commitment: receiver_commitment(&other_salt, &fixture_receiver()),
                ..fixture
            }),
        ),
        m(
            "a note of another value",
            |w| {
                w.note_value -= 1;
                with_memo(w, payment_memo(&fixture_payment_ref()));
            },
            Rebound(Binding {
                zatoshi: FIXTURE_VALUE - 1,
                ..fixture
            }),
        ),
    ]);
    catalog
}
