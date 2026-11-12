//! The amount-hidden payment statement (journal version 2).
//!
//! Everything [`verify_on`] checks, with the public amount replaced by a masked value. An app that keeps the amount
//! encrypted (for example under FHE) publishes `M = Z + R (mod 2^64)` instead of the amount Z it expects; R is known
//! only to the payer and the receiver, so M says nothing about Z. The proof shows that the note pays at least `M - R`:
//!
//! - `R = u64_be(keccak256("seam/amount-mask/v1" || salt32 || paymentRef32)[0..8])` ([`amount_mask`]). The salt is the
//!   one the receiver commitment `keccak256(salt32 || receiver43)` binds, and `paymentRef` is the one the memo names,
//!   so the app's check of those two journal fields fixes R.
//! - The note's value must be at least `M.wrapping_sub(R)` ([`verify_masked_on`]).
//!
//! The journal ([`MaskedPaymentJournal::abi_encode`], 192 bytes) is `abi.encode(uint8 version = 2, uint32 branchId,
//! bytes32 anchor, bytes32 paymentRef, bytes32 receiverCommitment, uint64 maskedAmount)`: version 1's layout with the
//! amount's word replaced by M. It holds no amount, no txid, no note commitment, no nullifier and no address.
//!
//! What the app must do: require the journal's `paymentRef`, `receiverCommitment` and `maskedAmount` to equal its own
//! (the M it published). A wrong R can only come from whoever supplied the encrypted mask, the receiving side; it
//! either raises the threshold above Z (an honest payer, who checks `M - Z == R` before paying, doesn't pay) or lowers
//! it (the receiver can be underpaid): both only hurt the receiving side.

use serde::{Deserialize, Serialize};

use crate::{Error, JOURNAL_LENGTH, NETWORK, Network, PaymentWitness, keccak256, verify_on};

/// The first word of a masked payment journal.
pub const MASKED_JOURNAL_VERSION: u8 = 2;
/// Domain separator of the amount mask.
pub const MASK_DOMAIN: &[u8; 19] = b"seam/amount-mask/v1";

/// The private input of an amount-hidden payment proof: a payment witness and the masked value M the app published.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaskedPaymentInput {
    pub witness: PaymentWitness,
    /// M: the app's masked value. Public: it is the journal's last word.
    pub masked_amount: u64,
}

/// M alone in a debug print: the witness holds the payment's transaction, the note's opening and the salt.
impl core::fmt::Debug for MaskedPaymentInput {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("MaskedPaymentInput")
            .field("masked_amount", &self.masked_amount)
            .finish_non_exhaustive()
    }
}

/// The public output of an amount-hidden payment proof (the zkVM journal).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaskedPaymentJournal {
    /// Always [`MASKED_JOURNAL_VERSION`].
    pub version: u8,
    pub branch_id: u32,
    /// SHA-256d of header T, internal byte order (as the relay stores it).
    pub anchor: [u8; 32],
    pub payment_ref: [u8; 32],
    pub receiver_commitment: [u8; 32],
    /// M, as the app published it.
    pub masked_amount: u64,
}

impl MaskedPaymentJournal {
    /// `abi.encode(uint8 version, uint32 branchId, bytes32 anchor, bytes32 paymentRef, bytes32 receiverCommitment,
    /// uint64 maskedAmount)`, what `ZcashMaskedPaymentVerifier.verify` decodes.
    pub fn abi_encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(JOURNAL_LENGTH);
        out.extend_from_slice(&word(u64::from(self.version)));
        out.extend_from_slice(&word(u64::from(self.branch_id)));
        out.extend_from_slice(&self.anchor);
        out.extend_from_slice(&self.payment_ref);
        out.extend_from_slice(&self.receiver_commitment);
        out.extend_from_slice(&word(self.masked_amount));
        out
    }
}

fn word(v: u64) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[24..].copy_from_slice(&v.to_be_bytes());
    w
}

/// R: the first 8 bytes, big-endian, of `keccak256("seam/amount-mask/v1" || salt32 || paymentRef32)`.
pub fn amount_mask(salt: &[u8; 32], payment_ref: &[u8; 32]) -> u64 {
    let mut preimage = [0u8; 19 + 32 + 32];
    preimage[..19].copy_from_slice(MASK_DOMAIN);
    preimage[19..51].copy_from_slice(salt);
    preimage[51..].copy_from_slice(payment_ref);
    let h = keccak256(&preimage);
    u64::from_be_bytes([h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]])
}

/// M = Z + R (mod 2^64): what the receiving side computes for the amount Z it expects.
pub fn masked_amount(amount: u64, salt: &[u8; 32], payment_ref: &[u8; 32]) -> u64 {
    amount.wrapping_add(amount_mask(salt, payment_ref))
}

/// M - R (mod 2^64): the least value a note must carry to prove against M.
pub fn masked_threshold(masked: u64, salt: &[u8; 32], payment_ref: &[u8; 32]) -> u64 {
    masked.wrapping_sub(amount_mask(salt, payment_ref))
}

/// Why an input does not prove an amount-hidden payment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaskedError {
    /// The witness fails the payment statement itself.
    Payment(Error),
    /// The note's value is below `M - R`.
    Underpaid,
}

/// Checks the input on this build's network and returns the public output, or why it fails.
pub fn verify_masked(input: &MaskedPaymentInput) -> Result<MaskedPaymentJournal, MaskedError> {
    verify_masked_on(&NETWORK, input)
}

/// [`verify_masked`] on network `net`: the payment statement, then `value >= M - R`.
pub fn verify_masked_on(net: &Network, input: &MaskedPaymentInput) -> Result<MaskedPaymentJournal, MaskedError> {
    let j = verify_on(net, &input.witness).map_err(MaskedError::Payment)?;
    if j.zatoshi < masked_threshold(input.masked_amount, &input.witness.salt, &j.payment_ref) {
        return Err(MaskedError::Underpaid);
    }
    Ok(MaskedPaymentJournal {
        version: MASKED_JOURNAL_VERSION,
        branch_id: j.branch_id,
        anchor: j.anchor,
        payment_ref: j.payment_ref,
        receiver_commitment: j.receiver_commitment,
        masked_amount: input.masked_amount,
    })
}
