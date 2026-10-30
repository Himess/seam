//! The Seam payment statement: what a payment proof proves (docs/PAYMENTS.md).
//!
//! A proof shows that a shielded Ironwood payment was made: to a receiver committed to as
//! `keccak256(salt32 || receiver43)`, of a value, for one `paymentRef`, in a block whose chain history a later header
//! T commits to. Plain Rust, shared by the zkVM guest (which proves that [`verify`] accepts a private
//! [`PaymentWitness`]) and the hosts (which build witnesses and run the same check natively). Adapted from Zecured's
//! payment statement (`tier2/payment-statement` at 73de9f909e270e8aea10c2b221905f62a6a4c4df): the memo binds a text
//! reference any wallet can send instead of Zecured's binary order id, and the journal's first word is a version.
//!
//! **The network is a build parameter, never a witness field:** a build proves payments on one Zcash network
//! ([`NETWORK`]), Testnet by default and Mainnet with the cargo feature `mainnet`, which fixes the epochs a proof may
//! walk. Each network has its own guest and image id (`payment-test`, `payment-main`).
//!
//! [`verify`] accepts a witness only if:
//!
//! 1. **Epoch:** its branch id is one the build allows, and the transaction's own branch id is it.
//! 2. **Shape:** the transaction is v6 and Ironwood-only with outputs enabled: no transparent input or output, and no
//!    Sapling or Orchard-pool bundle, even an empty one.
//! 3. **Inclusion:** the txid's path reaches B's `merkleRoot`; B's ZIP 221 leaf, through the path and the peaks with
//!    T's auth data root, reaches T's `hashBlockCommitments` (ZIP 244); every node is personalized with the epoch's
//!    branch id.
//! 4. **The note:** the opening rebuilds a V3 note (ZIP 2005) whose rho is the action's nullifier, and the action's
//!    ciphertext, recovered with the ephemeral secret derived from that note and its `pk_d`, yields exactly that
//!    note. So the receiver's wallet finds it and can spend it.
//! 5. **The memo:** its first 70 bytes are the ASCII text `seam1:` and 64 lowercase hex digits, which decode to
//!    `paymentRef`. The rest of the memo is free.
//! 6. **The receiver commitment:** `keccak256(salt32 || receiver43)`.
//!
//! The journal ([`PaymentJournal::abi_encode`], 192 bytes) is `abi.encode(uint8 version = 1, uint32 branchId,
//! bytes32 anchor, bytes32 paymentRef, bytes32 receiverCommitment, uint64 zatoshi)`. It never contains a txid, a note
//! commitment, a nullifier or an address.

use orchard::{
    Address, Note,
    note::{NoteVersion, RandomSeed, Rho},
    note_encryption::IronwoodDomain,
    value::NoteValue,
};
use serde::{Deserialize, Serialize};
use zcash_note_encryption::{Domain, try_output_recovery_with_pkd_esk};
use zcash_primitives::transaction::Transaction;
use zcash_protocol::consensus::BranchId;

pub mod chain;
#[cfg(feature = "k4")]
pub mod k4;

pub use chain::{
    HEADER_LENGTH, Included, Inclusion, MAINNET, MmrStep, NETWORK, NU6_3_BRANCH_ID, NU7_BRANCH_ID, Network, TESTNET,
    block_commitments, check_inclusion, check_payment_transaction_on, keccak256, sha256d, txid_path_root,
};

/// The first word of every payment journal: its version.
pub const JOURNAL_VERSION: u8 = 1;
/// Length of a payment journal: six ABI words.
pub const JOURNAL_LENGTH: usize = 6 * 32;
/// The text a payment's memo opens with.
pub const MEMO_PREFIX: &[u8; 6] = b"seam1:";
/// Bytes of the memo the statement reads: the prefix and the 64 hex digits of `paymentRef`.
pub const MEMO_BINDING_LENGTH: usize = MEMO_PREFIX.len() + 64;
/// Length of a note memo (orchard's `Domain::Memo` of the Ironwood domain).
pub const MEMO_LENGTH: usize = 512;

/// The private input of a payment proof.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentWitness {
    /// Consensus branch ID of the epoch of block B and header T (one the build allows).
    pub branch_id: u32,
    /// The payment transaction, serialized (v6).
    pub tx: Vec<u8>,
    /// Index of the Ironwood action that creates the payment note.
    pub action_index: u32,
    /// The payment note's receiver (43-byte raw Orchard-protocol receiver), value and rseed: what the payer kept from
    /// building the transaction, or what the receiver recovered with its viewing key.
    pub note_recipient: Vec<u8>,
    pub note_value: u64,
    pub note_rseed: [u8; 32],
    /// The salt of the receiver commitment `keccak256(salt32 || receiver43)`.
    pub salt: [u8; 32],
    /// The transaction's position in block B and the SHA-256d Merkle path from its txid.
    pub tx_index: u32,
    pub txid_path: Vec<[u8; 32]>,
    /// Block B's header.
    pub block_header: Vec<u8>,
    /// Block B's history-tree leaf (ZIP 221 V3, serialized), its path to a peak, all peaks (oldest first) and which
    /// peak the path reaches.
    pub leaf: Vec<u8>,
    pub mmr_path: Vec<MmrStep>,
    pub peaks: Vec<Vec<u8>>,
    pub peak_index: u32,
    /// The later header T and its block's auth data root (ZIP 244).
    pub tip_header: Vec<u8>,
    pub tip_auth_data_root: [u8; 32],
}

/// The epoch alone in a debug print, which may end up in a log: a witness holds the payment's transaction, the note's
/// opening and the salt.
impl core::fmt::Debug for PaymentWitness {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PaymentWitness").field("branch_id", &self.branch_id).finish_non_exhaustive()
    }
}

/// The public output of a payment proof (the zkVM journal).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentJournal {
    /// Always [`JOURNAL_VERSION`].
    pub version: u8,
    /// Consensus branch ID of the epoch the proof walked.
    pub branch_id: u32,
    /// SHA-256d of header T, internal byte order (as the relay stores it).
    pub anchor: [u8; 32],
    /// The reference the memo names.
    pub payment_ref: [u8; 32],
    /// `keccak256(salt32 || receiver43)` of the note's receiver.
    pub receiver_commitment: [u8; 32],
    /// The note's value, in zatoshi.
    pub zatoshi: u64,
}

impl PaymentJournal {
    /// `abi.encode(uint8 version, uint32 branchId, bytes32 anchor, bytes32 paymentRef, bytes32 receiverCommitment,
    /// uint64 zatoshi)`, what `ZcashPaymentVerifier.verify` decodes.
    pub fn abi_encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(JOURNAL_LENGTH);
        out.extend_from_slice(&word(u64::from(self.version)));
        out.extend_from_slice(&word(u64::from(self.branch_id)));
        out.extend_from_slice(&self.anchor);
        out.extend_from_slice(&self.payment_ref);
        out.extend_from_slice(&self.receiver_commitment);
        out.extend_from_slice(&word(self.zatoshi));
        out
    }
}

/// An unsigned integer as one ABI word.
fn word(v: u64) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[24..].copy_from_slice(&v.to_be_bytes());
    w
}

/// The receiver commitment the EVM chain sees: `keccak256(salt32 || receiver43)`.
pub fn receiver_commitment(salt: &[u8; 32], receiver: &[u8; 43]) -> [u8; 32] {
    let mut preimage = [0u8; 32 + 43];
    preimage[..32].copy_from_slice(salt);
    preimage[32..].copy_from_slice(receiver);
    keccak256(&preimage)
}

const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

/// The memo text that binds a payment to `payment_ref`: `seam1:` and its 64 lowercase hex digits. A ZIP 302 text
/// memo, which wallets show as text and can send typed by hand.
pub fn memo_text(payment_ref: &[u8; 32]) -> String {
    let mut text = String::with_capacity(MEMO_BINDING_LENGTH);
    text.push_str("seam1:");
    for b in payment_ref {
        text.push(char::from(HEX_DIGITS[usize::from(b >> 4)]));
        text.push(char::from(HEX_DIGITS[usize::from(b & 0x0f)]));
    }
    text
}

/// The 512-byte memo of a payment for `payment_ref`: [`memo_text`], zero-padded as ZIP 302 pads a text memo.
pub fn payment_memo(payment_ref: &[u8; 32]) -> [u8; MEMO_LENGTH] {
    let mut memo = [0u8; MEMO_LENGTH];
    memo[..MEMO_BINDING_LENGTH].copy_from_slice(memo_text(payment_ref).as_bytes());
    memo
}

fn hex_digit(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    }
}

/// The `paymentRef` a memo binds: its first 70 bytes must be `seam1:` and 64 lowercase hex digits. Uppercase digits
/// are refused, so each reference has exactly one memo binding.
pub fn memo_payment_ref(memo: &[u8; MEMO_LENGTH]) -> Option<[u8; 32]> {
    if &memo[..MEMO_PREFIX.len()] != MEMO_PREFIX {
        return None;
    }
    let (pairs, _) = memo[MEMO_PREFIX.len()..MEMO_BINDING_LENGTH].as_chunks::<2>();
    let mut payment_ref = [0u8; 32];
    for (byte, [hi, lo]) in payment_ref.iter_mut().zip(pairs) {
        *byte = (hex_digit(*hi)? << 4) | hex_digit(*lo)?;
    }
    Some(payment_ref)
}

/// Why a witness does not prove a payment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The witness's branch id is not one of the epochs this build proves.
    BranchNotAllowed,
    /// The transaction does not parse.
    Transaction,
    /// Not a v6 transaction.
    NotV6,
    /// The transaction's consensus branch id is not one the build allows, or not the witness's epoch.
    WrongEpoch,
    /// Not an Ironwood-only transaction: a transparent, Sapling or Orchard-pool bundle, or no Ironwood bundle.
    NotIronwoodOnly,
    /// The Ironwood bundle does not enable outputs.
    OutputsDisabled,
    NoSuchAction,
    /// B's or T's header is not 1487 bytes.
    Header,
    TxidNotInBlock,
    LeafIsNotTheBlock,
    HistoryPath,
    HistoryNotCommitted,
    /// The opening is not a note (receiver not a valid 43-byte receiver, rho or rseed invalid).
    Note,
    /// The action's ciphertext does not decrypt to a note under the opening's ephemeral secret.
    NotDecryptable,
    /// The recovered note is not the opened one.
    NoteCommitmentMismatch,
    /// The note plaintext is not a V3 (ZIP 2005) Ironwood note.
    NoteVersion,
    /// The note's memo does not open with `seam1:` and 64 lowercase hex digits.
    Memo,
}

/// Checks the witness on this build's network and returns the public output, or why it fails.
pub fn verify(w: &PaymentWitness) -> Result<PaymentJournal, Error> {
    verify_on(&NETWORK, w)
}

/// [`verify`] on network `net` (the guests only ever use [`NETWORK`]; hosts and tests may check a witness against
/// either network).
pub fn verify_on(net: &Network, w: &PaymentWitness) -> Result<PaymentJournal, Error> {
    // 1. The epoch: one this build proves.
    if !net.allows(w.branch_id) {
        return Err(Error::BranchNotAllowed);
    }
    let branch = BranchId::try_from(w.branch_id).map_err(|_| Error::BranchNotAllowed)?;

    // 2. The transaction: v6, Ironwood only, outputs enabled. (v5 and v6 transactions carry their own branch id; the
    //    one passed to `read` only matters for older versions.)
    let tx = Transaction::read(&w.tx[..], branch).map_err(|_| Error::Transaction)?;
    check_payment_transaction_on(net, &tx)?;
    let bundle = tx.ironwood_bundle().ok_or(Error::NotIronwoodOnly)?;
    let action = bundle.actions().get(w.action_index as usize).ok_or(Error::NoSuchAction)?;

    // 3. It is in block B, a leaf of the history committed to by header T, both of the witness's epoch; the
    //    transaction's own branch id is that epoch's.
    let included = check_inclusion(
        tx.txid().as_ref(),
        &Inclusion {
            branch_id: w.branch_id,
            tx_index: w.tx_index,
            txid_path: &w.txid_path,
            block_header: &w.block_header,
            leaf: &w.leaf,
            mmr_path: &w.mmr_path,
            peaks: &w.peaks,
            peak_index: w.peak_index,
            tip_header: &w.tip_header,
            tip_auth_data_root: &w.tip_auth_data_root,
        },
    )?;
    if tx.consensus_branch_id() != branch {
        return Err(Error::WrongEpoch);
    }

    // 4. The note, rebuilt from the opening: recovering the action's ciphertext with the ephemeral secret derived from
    //    the opened note (and its pk_d) checks that the plaintext is a note whose commitment is the action's cmx and
    //    whose ephemeral key is the action's (ZIP 212). The recovered note is compared field by field with the opened
    //    one (`Note`'s `PartialEq` would compute both commitments again).
    let recipient: [u8; 43] = w.note_recipient.clone().try_into().map_err(|_| Error::Note)?;
    let address = Option::<Address>::from(Address::from_raw_address_bytes(&recipient)).ok_or(Error::Note)?;
    // An action's output note has rho = the nullifier spent by the same action.
    let rho = Option::<Rho>::from(Rho::from_bytes(&action.nullifier().to_bytes())).ok_or(Error::Note)?;
    let rseed = Option::<RandomSeed>::from(RandomSeed::from_bytes(w.note_rseed, &rho)).ok_or(Error::Note)?;
    let note =
        Option::<Note>::from(Note::from_parts(address, NoteValue::from_raw(w.note_value), rho, rseed, NoteVersion::V3))
            .ok_or(Error::Note)?;
    let domain = IronwoodDomain::for_action(action);
    let esk = IronwoodDomain::derive_esk(&note).ok_or(Error::NotDecryptable)?;
    let pk_d = IronwoodDomain::get_pk_d(&note);
    let (recovered, _, memo) =
        try_output_recovery_with_pkd_esk(&domain, pk_d, esk, action).ok_or(Error::NotDecryptable)?;
    if recovered.recipient().to_raw_address_bytes() != recipient
        || recovered.value() != note.value()
        || recovered.rseed().as_bytes() != note.rseed().as_bytes()
        || recovered.rho().to_bytes() != note.rho().to_bytes()
    {
        return Err(Error::NoteCommitmentMismatch);
    }
    if recovered.version() != NoteVersion::V3 {
        return Err(Error::NoteVersion);
    }

    // 5. The memo names the reference.
    let payment_ref = memo_payment_ref(&memo).ok_or(Error::Memo)?;

    Ok(PaymentJournal {
        version: JOURNAL_VERSION,
        branch_id: w.branch_id,
        anchor: included.tip_hash,
        payment_ref,
        receiver_commitment: receiver_commitment(&w.salt, &recipient),
        zatoshi: w.note_value,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h32(s: &str) -> [u8; 32] {
        let b: Vec<u8> = (0..64).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect();
        b.try_into().unwrap()
    }

    #[test]
    fn journal_abi_layout() {
        let j = PaymentJournal {
            version: JOURNAL_VERSION,
            branch_id: NU7_BRANCH_ID,
            anchor: [1; 32],
            payment_ref: [2; 32],
            receiver_commitment: [3; 32],
            zatoshi: 0x0102_0304_0506,
        };
        let e = j.abi_encode();
        assert_eq!(e.len(), JOURNAL_LENGTH);
        assert_eq!(e.len(), 192, "six words");
        assert_eq!(&e[0..31], &[0; 31]);
        assert_eq!(e[31], 1, "version");
        assert_eq!(&e[32..60], &[0; 28]);
        assert_eq!(&e[60..64], &[0x77, 0x19, 0x0a, 0xd9], "branch id, big-endian");
        assert_eq!(&e[64..96], &[1; 32], "anchor");
        assert_eq!(&e[96..128], &[2; 32], "paymentRef");
        assert_eq!(&e[128..160], &[3; 32], "receiver commitment");
        assert_eq!(&e[160..186], &[0; 26]);
        assert_eq!(&e[186..192], &[1, 2, 3, 4, 5, 6], "zatoshi, big-endian");
    }

    #[test]
    fn the_memo_text_is_seam1_and_lowercase_hex() {
        let r = h32("00ff10ab00000000000000000000000000000000000000000000000000000000");
        assert_eq!(memo_text(&r), "seam1:00ff10ab00000000000000000000000000000000000000000000000000000000");
        let memo = payment_memo(&r);
        assert_eq!(&memo[..MEMO_BINDING_LENGTH], memo_text(&r).as_bytes());
        assert!(memo[MEMO_BINDING_LENGTH..].iter().all(|b| *b == 0), "zero-padded, as ZIP 302 pads a text memo");
        assert!(memo[0] <= 0xf4, "a ZIP 302 text memo");
        assert_eq!(memo_payment_ref(&memo), Some(r));
    }

    #[test]
    fn the_memo_binds_one_reference() {
        let r = [0xab; 32];
        let memo = payment_memo(&r);
        // Text after the binding is free.
        let mut annotated = memo;
        annotated[MEMO_BINDING_LENGTH..MEMO_BINDING_LENGTH + 11].copy_from_slice(b" order 1234");
        assert_eq!(memo_payment_ref(&annotated), Some(r));
        // Uppercase hex digits are refused: one reference, one binding.
        let mut upper = memo;
        upper[6] = b'A';
        assert_eq!(memo_payment_ref(&upper), None);
        // Any non-hex digit, another prefix, a missing digit.
        let mut not_hex = memo;
        not_hex[69] = b'g';
        assert_eq!(memo_payment_ref(&not_hex), None);
        let mut other = memo;
        other[4] = b'2';
        assert_eq!(memo_payment_ref(&other), None);
        let mut short = memo;
        short[69] = 0;
        assert_eq!(memo_payment_ref(&short), None);
        // The ZIP 302 empty memo (0xF6 then zeros) and the all-zero memo bind nothing.
        let mut empty = [0u8; MEMO_LENGTH];
        empty[0] = 0xf6;
        assert_eq!(memo_payment_ref(&empty), None);
        assert_eq!(memo_payment_ref(&[0u8; MEMO_LENGTH]), None);
    }

    /// `keccak256(salt32 || receiver43)` over 32 bytes of 0x11 then 43 bytes of 0x22, the vector Foundry's
    /// `cast keccak` gives.
    #[test]
    fn receiver_commitment_is_keccak_of_salt_and_receiver() {
        let c = receiver_commitment(&[0x11; 32], &[0x22; 43]);
        assert_eq!(c, h32("bae7b4976157459acb192cab961fec4003a1c8714e9cdee2a2bfa7f779ef6e10"));
        assert_ne!(c, receiver_commitment(&[0x12; 32], &[0x22; 43]));
        assert_ne!(c, receiver_commitment(&[0x11; 32], &[0x23; 43]));
    }

    #[test]
    fn the_networks_and_their_epochs() {
        assert_eq!(TESTNET.branch_ids, &[NU7_BRANCH_ID]);
        assert_eq!(MAINNET.branch_ids, &[NU6_3_BRANCH_ID, NU7_BRANCH_ID]);
        #[cfg(not(feature = "mainnet"))]
        assert_eq!(NETWORK, TESTNET);
        #[cfg(feature = "mainnet")]
        assert_eq!(NETWORK, MAINNET);
    }
}
