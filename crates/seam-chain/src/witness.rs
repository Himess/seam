//! The private input of a payment proof (`seam_payment::PaymentWitness`): a located transaction
//! ([`crate::locate`]) and its payment note's opening, which the payer kept when it built the transaction, or which
//! the receiver finds with its viewing key by trial decryption on this machine. Each witness is checked with the
//! statement natively before it is used. Ported from Zecured's `payment.rs` and `witness::build`
//! (`signer/crates/deposit-witness/src` at 73de9f909e270e8aea10c2b221905f62a6a4c4df).
//!
//! A witness and an opening hold the note's receiver, and a witness the receiver commitment's salt: they stay on the
//! machines of whoever builds and proves them, and never reach a log.

use anyhow::{Context, Result, anyhow};
use orchard::{
    keys::{FullViewingKey, Scope},
    note_encryption::IronwoodDomain,
};
use seam_payment::{PaymentJournal, PaymentWitness, memo_payment_ref, receiver_commitment, verify_on};
use serde::{Deserialize, Serialize};
use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_note_encryption::try_note_decryption;
use zcash_primitives::transaction::Transaction;

use crate::{locate::Located, network::ZcashNetwork};

/// The opening of a payment note: the action that creates it, and the note's receiver, value and rseed (its rho is
/// the nullifier the same action spends).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Opening {
    /// The payment transaction's id, as block explorers show it, when the payer recorded it with the opening (`seam
    /// pay` does): a txid stays out of logs, so the opening file carries it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub txid: Option<String>,
    /// The action's index in the transaction's Ironwood bundle.
    pub action_index: u32,
    /// The 43-byte raw Orchard-protocol receiver (hex).
    #[serde(with = "hex::serde")]
    pub receiver: [u8; 43],
    /// The value, in zatoshi.
    pub value: u64,
    /// The rseed (hex).
    #[serde(with = "hex::serde")]
    pub rseed: [u8; 32],
}

/// No receiver or rseed in a debug print, which may end up in a log.
impl std::fmt::Debug for Opening {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Opening")
            .field("action_index", &self.action_index)
            .field("value", &self.value)
            .finish()
    }
}

impl Opening {
    /// The receiver commitment `keccak256(salt32 || receiver43)` the payment pays under `salt`.
    pub fn receiver_commitment(&self, salt: &[u8; 32]) -> [u8; 32] {
        receiver_commitment(salt, &self.receiver)
    }
}

/// The witness of the payment `at` holds, as `opening` opens it, with the receiver commitment's salt `salt`, and its
/// journal: checked with the statement of the network the transaction was read from.
pub fn from_opening(at: &Located, opening: &Opening, salt: [u8; 32]) -> Result<(PaymentWitness, PaymentJournal)> {
    let witness = PaymentWitness {
        branch_id: at.epoch.branch_id,
        tx: at.raw_tx.clone(),
        action_index: opening.action_index,
        note_recipient: opening.receiver.to_vec(),
        note_value: opening.value,
        note_rseed: opening.rseed,
        salt,
        tx_index: u32::try_from(at.tx_index)?,
        txid_path: at.txid_path.clone(),
        block_header: at.block_header.clone(),
        leaf: at.leaf.clone(),
        mmr_path: at.mmr_path.clone(),
        peaks: at.peaks.clone(),
        peak_index: u32::try_from(at.peak_index)?,
        tip_header: at.tip_header.clone(),
        tip_auth_data_root: at.tip_auth_data_root,
    };
    let journal = verify_on(at.network.statement(), &witness)
        .map_err(|e| anyhow!("the payment statement refuses the witness: {e:?}"))?;
    Ok((witness, journal))
}

/// A payment a viewing key finds in a transaction: its opening and the reference its memo names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub opening: Opening,
    pub payment_ref: [u8; 32],
}

/// The payments to `fvk` in transaction `tx`: the notes of its Ironwood actions that `fvk`'s external incoming
/// viewing key decrypts (trial decryption, here) and whose memo names a payment reference (`seam1:` and 64 lowercase
/// hex digits), in action order.
pub fn find_payments(tx: &Transaction, fvk: &FullViewingKey) -> Result<Vec<Found>> {
    let ivk = fvk.to_ivk(Scope::External).prepare();
    let bundle = tx.ironwood_bundle().context("the transaction has no Ironwood bundle")?;
    let mut out = vec![];
    for (i, action) in bundle.actions().iter().enumerate() {
        let Some((note, _, memo)) = try_note_decryption(&IronwoodDomain::for_action(action), &ivk, action) else {
            continue;
        };
        if let Some(payment_ref) = memo_payment_ref(&memo) {
            out.push(Found {
                opening: Opening {
                    txid: None,
                    action_index: u32::try_from(i)?,
                    receiver: note.recipient().to_raw_address_bytes(),
                    value: note.value().inner(),
                    rseed: *note.rseed().as_bytes(),
                },
                payment_ref,
            });
        }
    }
    Ok(out)
}

/// The Orchard-protocol full viewing key of a UFVK of Zcash `network`: after NU6.3 it views the Ironwood pool.
pub fn ufvk_fvk(network: ZcashNetwork, ufvk: &str) -> Result<FullViewingKey> {
    UnifiedFullViewingKey::decode(&network.params(), ufvk.trim())
        .map_err(|e| anyhow!("not a unified full viewing key of Zcash {network}: {e}"))?
        .orchard()
        .cloned()
        .context("the viewing key has no Orchard-protocol (Ironwood) key")
}

#[cfg(test)]
mod tests {
    use orchard::keys::SpendingKey;
    use seam_payment::k4::{
        FIXTURE_RECEIVER_SPENDING_KEY, FIXTURE_RSEED, FIXTURE_SALT, FIXTURE_VALUE, OTHER_RECEIVER_SPENDING_KEY,
        PublicTx, fixture_payment_ref, fixture_receiver, synthesize,
    };
    use zcash_protocol::consensus::BranchId;

    use super::*;
    use crate::network::Epoch;

    const PUBLIC_TX: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../seam-payment/tests/fixtures/TEST-ONLY_testnet_ironwood_tx.json");

    /// The synthetic payment witness of seam-payment's K4 (TEST-ONLY), as a located transaction.
    fn base() -> (PaymentWitness, Located) {
        let p: PublicTx = serde_json::from_slice(&std::fs::read(PUBLIC_TX).unwrap()).unwrap();
        let w = synthesize(&p);
        let at = Located {
            network: ZcashNetwork::Testnet,
            epoch: ZcashNetwork::Testnet.epoch(4_488_560).unwrap(),
            raw_tx: w.tx.clone(),
            height: 4_488_543,
            tx_index: w.tx_index as usize,
            block_transactions: 2,
            txid_path: w.txid_path.clone(),
            block_header: w.block_header.clone(),
            leaf: w.leaf.clone(),
            leaf_index: 23_517,
            leaves: 23_534,
            mmr_path: w.mmr_path.clone(),
            peaks: w.peaks.clone(),
            peak_index: w.peak_index as usize,
            tip_height: 4_488_560,
            tip_header: w.tip_header.clone(),
            tip_auth_data_root: w.tip_auth_data_root,
        };
        (w, at)
    }

    fn fvk(sk: [u8; 32]) -> FullViewingKey {
        FullViewingKey::from(&SpendingKey::from_bytes(sk).unwrap())
    }

    fn fixture_opening(action_index: u32) -> Opening {
        Opening {
            txid: None,
            action_index,
            receiver: fixture_receiver(),
            value: FIXTURE_VALUE,
            rseed: FIXTURE_RSEED,
        }
    }

    /// The payer's opening gives back the fixture's witness and journal; another opening is refused.
    #[test]
    fn a_witness_from_the_payers_opening() {
        let (w, at) = base();
        let (built, journal) = from_opening(&at, &fixture_opening(w.action_index), FIXTURE_SALT).unwrap();
        assert_eq!(built, w);
        assert_eq!(journal, verify_on(&seam_payment::TESTNET, &w).unwrap());
        assert_eq!(journal.payment_ref, fixture_payment_ref());
        assert_eq!(journal.receiver_commitment, fixture_opening(0).receiver_commitment(&FIXTURE_SALT));
        let mut wrong = fixture_opening(w.action_index);
        wrong.rseed[0] ^= 1;
        assert!(from_opening(&at, &wrong, FIXTURE_SALT).is_err());
        assert!(from_opening(&at, &fixture_opening(w.action_index ^ 1), FIXTURE_SALT).is_err());
        // Another epoch's branch id: the statement refuses it.
        let mut other = at.clone();
        other.epoch = Epoch {
            branch_id: seam_payment::NU6_3_BRANCH_ID,
            ..other.epoch
        };
        assert!(from_opening(&other, &fixture_opening(w.action_index), FIXTURE_SALT).is_err());
    }

    /// The receiver's viewing key finds the same opening by trial decryption, and through a UFVK; another key finds
    /// nothing.
    #[test]
    fn the_receiver_finds_the_opening_with_its_viewing_key() {
        let (w, at) = base();
        let tx = Transaction::read(&w.tx[..], BranchId::Nu7).unwrap();
        let found = find_payments(&tx, &fvk(FIXTURE_RECEIVER_SPENDING_KEY)).unwrap();
        assert_eq!(
            found,
            vec![Found {
                opening: fixture_opening(w.action_index),
                payment_ref: fixture_payment_ref(),
            }]
        );
        assert!(find_payments(&tx, &fvk(OTHER_RECEIVER_SPENDING_KEY)).unwrap().is_empty());
        let (built, _) = from_opening(&at, &found[0].opening, FIXTURE_SALT).unwrap();
        assert_eq!(built, w);

        // A UFVK of the test key (it holds no funds), decoded on its own network only.
        let ufvk = UnifiedFullViewingKey::from_orchard_fvk(fvk(FIXTURE_RECEIVER_SPENDING_KEY))
            .unwrap()
            .encode(&ZcashNetwork::Testnet.params())
            .unwrap();
        let decoded = ufvk_fvk(ZcashNetwork::Testnet, &format!("{ufvk}\n")).unwrap();
        assert_eq!(decoded.to_bytes(), fvk(FIXTURE_RECEIVER_SPENDING_KEY).to_bytes());
        assert!(ufvk_fvk(ZcashNetwork::Mainnet, &ufvk).is_err());
        assert!(ufvk_fvk(ZcashNetwork::Testnet, "uviewtest1notakey").is_err());
    }

    /// An opening file is JSON with hex fields, and its debug print shows neither the receiver nor the rseed.
    #[test]
    fn openings_round_trip_without_printing_the_receiver() {
        let o = fixture_opening(1);
        let json = serde_json::to_value(&o).unwrap();
        assert_eq!(json["receiver"], hex::encode(fixture_receiver()));
        assert_eq!(json["rseed"], hex::encode(FIXTURE_RSEED));
        assert_eq!(json["value"], FIXTURE_VALUE);
        assert!(json.get("txid").is_none(), "no txid, no field");
        assert_eq!(serde_json::from_value::<Opening>(json).unwrap(), o);
        let with_txid = Opening {
            txid: Some("ab".repeat(32)),
            ..o.clone()
        };
        let json = serde_json::to_value(&with_txid).unwrap();
        assert_eq!(json["txid"], "ab".repeat(32));
        assert_eq!(serde_json::from_value::<Opening>(json).unwrap(), with_txid);
        let shown = format!("{with_txid:?}");
        assert!(!shown.contains(&hex::encode(fixture_receiver())) && !shown.contains("receiver"));
        assert!(!shown.contains("abab"), "no txid in a debug print");
    }
}
