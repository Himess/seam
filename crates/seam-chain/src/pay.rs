//! A Seam payment from a disposable wallet: one Ironwood note of the payment's value to the receiver, with the memo of
//! its `paymentRef` and `ovk = None`, change to the wallet's internal address, and the ZIP 317 fee. Ironwood only, so
//! the statement can prove it (docs/PAYMENTS.md § 4). Ported from Zecured's `swap/core/src/pay.rs` and
//! `signer/crates/zcash-ops/src/spend.rs` (73de9f909e270e8aea10c2b221905f62a6a4c4df), which paid real Testnet orders.
//!
//! With `ovk = None` nobody, the payer included, can recover the note from the chain, so [`build_payment`] keeps its
//! opening from the PCZT, and [`check_payment`] runs the statement's checks that need no inclusion on the final
//! transaction: a payment that could not be proven is never sent.

use anyhow::{Context, Result, anyhow, bail, ensure};
use orchard::{
    Address, Note,
    circuit::{OrchardCircuitVersion, ProvingKey},
    keys::{FullViewingKey, Scope, SpendAuthorizingKey, SpendingKey},
    note::{NoteVersion, RandomSeed, Rho},
    note_encryption::IronwoodDomain,
    value::NoteValue,
};
use pczt::{
    Pczt,
    roles::{
        creator::Creator, io_finalizer::IoFinalizer, prover::Prover, signer::Signer, tx_extractor::TransactionExtractor,
    },
};
use seam_payment::{Network, check_payment_transaction_on, memo_payment_ref, payment_memo};
use zcash_note_encryption::{Domain, try_output_recovery_with_pkd_esk};
use zcash_primitives::transaction::{
    Transaction,
    builder::{BuildConfig, Builder, BundlePadding},
    fees::zip317,
};
use zcash_protocol::{
    consensus::{BlockHeight, Parameters},
    memo::MemoBytes,
    value::Zatoshis,
};

use crate::{scan::ReceivedNote, witness::Opening};

/// Blocks between a payment's target height and its expiry height: 120, about 50 minutes at NU7's 25-second blocks
/// (Zecured's own constant, so a change of the library's default never changes it silently).
pub const EXPIRY_DELTA: u32 = 120;

/// The ZIP 317 fee of an Ironwood bundle with `spends` spends and `outputs` outputs: 5,000 zatoshi per logical action,
/// at least 2, and an Orchard-family bundle has max(spends, outputs) logical actions.
pub fn zip317_fee(spends: usize, outputs: usize) -> u64 {
    const MARGINAL_FEE: u64 = 5_000;
    const GRACE_ACTIONS: u64 = 2;
    MARGINAL_FEE * (spends.max(outputs) as u64).max(GRACE_ACTIONS)
}

/// Unspent notes, oldest first, until they cover `amount` and the fee of a payment with change.
pub fn select_inputs<'a>(unspent: &[&'a ReceivedNote], amount: u64) -> Result<Vec<&'a ReceivedNote>> {
    let mut inputs = vec![];
    let mut total = 0u64;
    for n in unspent {
        if total >= amount.saturating_add(zip317_fee(inputs.len().max(1), 2)) {
            break;
        }
        total = total.saturating_add(n.value());
        inputs.push(*n);
    }
    ensure!(
        total >= amount.saturating_add(zip317_fee(inputs.len().max(1), 2)),
        "insufficient funds: {total} zatoshi for a payment of {amount}"
    );
    Ok(inputs)
}

/// An unauthorized payment PCZT, and what proving and recording it need.
pub struct UnsignedPayment {
    pub pczt: Pczt,
    /// The Ironwood action of each spend, in input order.
    pub spend_action_indices: Vec<usize>,
    /// The payment note's opening: keep it until the payment is proven.
    pub opening: Opening,
    pub fee: u64,
    pub change: u64,
}

/// Builds the payment PCZT at `target_height`: `value` zatoshi to `recipient` with the memo of `payment_ref` and
/// `ovk = None`, change to the wallet's internal address (with its internal OVK), the ZIP 317 fee, spending `inputs`
/// under `anchor`.
#[allow(clippy::too_many_arguments)]
pub fn build_payment<P: Parameters + Clone>(
    params: &P,
    target_height: BlockHeight,
    fvk: &FullViewingKey,
    anchor: orchard::Anchor,
    inputs: &[&ReceivedNote],
    recipient: Address,
    value: u64,
    payment_ref: &[u8; 32],
) -> Result<UnsignedPayment> {
    ensure!(!inputs.is_empty(), "no notes to spend");
    ensure!(value > 0, "a payment of nothing");
    let input_total = inputs
        .iter()
        .try_fold(0u64, |sum, n| sum.checked_add(n.value()))
        .context("the notes' values overflow")?;
    let fee_without_change = zip317_fee(inputs.len(), 1);
    let fee_with_change = zip317_fee(inputs.len(), 2);
    let needed = |fee: u64| value.checked_add(fee).context("the payment's value overflows");
    let (fee, change) = if input_total == needed(fee_without_change)? {
        (fee_without_change, 0)
    } else if input_total > needed(fee_with_change)? {
        (fee_with_change, input_total - value - fee_with_change)
    } else {
        bail!("insufficient funds: {input_total} zatoshi for a payment of {value} and a fee of {fee_with_change}");
    };

    let mut builder = Builder::new(
        params.clone(),
        target_height,
        BuildConfig::Standard {
            sapling_anchor: None,
            orchard_anchor: None,
            ironwood_anchor: Some(anchor),
            orchard_padding: BundlePadding::DEFAULT,
            ironwood_padding: BundlePadding::DEFAULT,
        },
    )
    .with_expiry_height(target_height + EXPIRY_DELTA);
    for input in inputs {
        builder
            .add_ironwood_spend::<zip317::FeeError>(fvk.clone(), input.note, input.merkle_path()?)
            .map_err(|e| anyhow!("add an Ironwood spend: {e}"))?;
    }
    let memo = MemoBytes::from_bytes(&payment_memo(payment_ref)).map_err(|e| anyhow!("the memo: {e}"))?;
    builder
        .add_ironwood_output::<zip317::FeeError>(
            None,
            recipient,
            Zatoshis::from_u64(value).map_err(|e| anyhow!("the value: {e:?}"))?,
            memo,
        )
        .map_err(|e| anyhow!("add the payment output: {e}"))?;
    if change > 0 {
        builder
            .add_ironwood_output::<zip317::FeeError>(
                Some(fvk.to_ovk(Scope::Internal)),
                fvk.address_at(0u32, Scope::Internal),
                Zatoshis::from_u64(change).map_err(|e| anyhow!("the change: {e:?}"))?,
                MemoBytes::empty(),
            )
            .map_err(|e| anyhow!("add the change output: {e}"))?;
    }
    let built = builder
        .build_for_pczt(rand::rng(), &zip317::FeeRule::standard())
        .map_err(|e| anyhow!("build the PCZT: {e}"))?;
    let spend_action_indices = (0..inputs.len())
        .map(|i| built.ironwood_meta.spend_action_index(i).context("a spend's action"))
        .collect::<Result<Vec<_>>>()?;
    let payment_action = built.ironwood_meta.output_action_index(0).context("the payment's action")?;
    let pczt = Creator::build_from_parts(built.pczt_parts).context("the PCZT creator")?;

    // The payment note's opening, which the PCZT keeps for the prover.
    let output = pczt.ironwood().actions().get(payment_action).context("the payment's action")?.output();
    ensure!(
        *output.recipient() == Some(recipient.to_raw_address_bytes()) && *output.value() == Some(value),
        "the PCZT's payment output is not the payment"
    );
    let opening = Opening {
        txid: None,
        action_index: u32::try_from(payment_action)?,
        receiver: recipient.to_raw_address_bytes(),
        value,
        rseed: (*output.rseed()).context("the PCZT does not keep the payment's rseed")?,
    };
    Ok(UnsignedPayment {
        pczt,
        spend_action_indices,
        opening,
        fee,
        change,
    })
}

/// The proving key of the post-NU6.3 circuit, which proves Ironwood actions. Building it takes a few seconds.
pub fn proving_key() -> ProvingKey {
    ProvingKey::build(OrchardCircuitVersion::PostNu6_3)
}

/// Proves the payment (Halo 2), signs its spends with the wallet's key and extracts the final transaction, whose
/// proofs and signatures the extractor verifies.
pub fn authorize(unsigned: UnsignedPayment, sk: &SpendingKey, pk: &ProvingKey) -> Result<Transaction> {
    let pczt = IoFinalizer::new(unsigned.pczt)
        .finalize_io(rand::rng())
        .map_err(|e| anyhow!("IO finalizer: {e:?}"))?;
    let pczt = Prover::new(pczt)
        .create_ironwood_proof(rand::rng(), pk)
        .map_err(|e| anyhow!("Ironwood prover: {e:?}"))?
        .finish();
    let mut signer = Signer::new(pczt).map_err(|e| anyhow!("signer: {e:?}"))?;
    let ask = SpendAuthorizingKey::from(sk);
    for i in &unsigned.spend_action_indices {
        signer.sign_ironwood(rand::rng(), *i, &ask).map_err(|e| anyhow!("sign a spend: {e:?}"))?;
    }
    TransactionExtractor::new(signer.finish())
        .extract(rand::rng())
        .map_err(|e| anyhow!("extract the transaction: {e:?}"))
}

/// The statement's checks that need no inclusion, on the final transaction (`seam_payment::verify`'s steps 1, 2, 4
/// and 5 for network `net`): a v6 Ironwood-only transaction of an epoch the network allows, whose action
/// `opening.action_index` carries exactly the opened V3 note, with the memo of `payment_ref`.
pub fn check_payment(net: &Network, tx: &Transaction, opening: &Opening, payment_ref: &[u8; 32]) -> Result<()> {
    check_payment_transaction_on(net, tx).map_err(|e| anyhow!("not a payment the statement accepts: {e:?}"))?;
    let bundle = tx.ironwood_bundle().context("no Ironwood bundle")?;
    let action = bundle.actions().get(opening.action_index as usize).context("no such action")?;
    let address =
        Option::<Address>::from(Address::from_raw_address_bytes(&opening.receiver)).context("the receiver")?;
    let rho = Option::<Rho>::from(Rho::from_bytes(&action.nullifier().to_bytes())).context("rho")?;
    let rseed = Option::<RandomSeed>::from(RandomSeed::from_bytes(opening.rseed, &rho)).context("rseed")?;
    let note = Option::<Note>::from(Note::from_parts(
        address,
        NoteValue::from_raw(opening.value),
        rho,
        rseed,
        NoteVersion::V3,
    ))
    .context("not a note")?;
    let esk = IronwoodDomain::derive_esk(&note).context("esk")?;
    let (recovered, _, memo) = try_output_recovery_with_pkd_esk(
        &IronwoodDomain::for_action(action),
        IronwoodDomain::get_pk_d(&note),
        esk,
        action,
    )
    .context("the payment's action does not decrypt to the opened note")?;
    ensure!(
        recovered.value() == note.value()
            && recovered.recipient().to_raw_address_bytes() == opening.receiver
            && recovered.rseed().as_bytes() == note.rseed().as_bytes()
            && recovered.version() == NoteVersion::V3,
        "the payment's action carries another note"
    );
    ensure!(memo_payment_ref(&memo) == Some(*payment_ref), "the payment's memo names another paymentRef");
    Ok(())
}

/// A transaction's consensus serialization, as it is sent.
pub fn serialize(tx: &Transaction) -> Result<Vec<u8>> {
    let mut data = vec![];
    tx.write(&mut data).context("serialize the transaction")?;
    Ok(data)
}

#[cfg(test)]
mod tests {
    use incrementalmerkletree::frontier::CommitmentTree;
    use orchard::{note::ExtractedNoteCommitment, tree::MerkleHashOrchard};
    use seam_payment::{
        TESTNET,
        k4::{FIXTURE_RECEIVER_SPENDING_KEY, fixture_payment_ref, fixture_receiver},
    };
    use zcash_protocol::consensus::Network as Params;

    use super::*;
    use crate::{
        scan::{IronwoodWitness, NotePosition},
        witness::find_payments,
    };

    /// The disposable test wallet's key: a test value that holds no funds.
    const WALLET_KEY: [u8; 32] = [0x09; 32];

    /// A wallet note of `value`, in a tree of a few other notes, with its witness and the tree's anchor.
    fn wallet_note(fvk: &FullViewingKey, value: u64) -> (ReceivedNote, orchard::Anchor) {
        let rho = Rho::from_bytes(&[3; 32]).unwrap();
        let rseed = (0u8..=255).find_map(|b| Option::from(RandomSeed::from_bytes([b; 32], &rho))).unwrap();
        let note = Note::from_parts(
            fvk.address_at(0u32, Scope::External),
            NoteValue::from_raw(value),
            rho,
            rseed,
            NoteVersion::V3,
        )
        .unwrap();
        let mut tree = CommitmentTree::<MerkleHashOrchard, 32>::empty();
        let other = |i: u8| MerkleHashOrchard::from_bytes(&[i; 32]).unwrap();
        for i in 1..4 {
            tree.append(other(i)).unwrap();
        }
        tree.append(MerkleHashOrchard::from_cmx(&ExtractedNoteCommitment::from(note.commitment()))).unwrap();
        let mut witness = IronwoodWitness::from_tree(tree.clone()).unwrap();
        for i in 4..6 {
            tree.append(other(i)).unwrap();
            witness.append(other(i)).unwrap();
        }
        let received = ReceivedNote {
            position: NotePosition {
                height: 4_500_000,
                tx_index: 1,
                action_index: 0,
            },
            nullifier: note.nullifier(fvk),
            note,
            scope: Scope::External,
            witness,
            spent_at: None,
        };
        (received, orchard::Anchor::from(tree.root()))
    }

    #[test]
    fn fees_and_inputs() {
        assert_eq!((1..=4).map(|o| zip317_fee(1, o)).collect::<Vec<_>>(), [10_000, 10_000, 15_000, 20_000]);
        let fvk = FullViewingKey::from(&SpendingKey::from_bytes(WALLET_KEY).unwrap());
        let (a, _) = wallet_note(&fvk, 30_000);
        let (b, _) = wallet_note(&fvk, 50_000);
        let notes = [&a, &b];
        assert_eq!(select_inputs(&notes, 15_000).unwrap().len(), 1);
        assert_eq!(select_inputs(&notes, 25_000).unwrap().len(), 2);
        assert!(select_inputs(&notes, 75_000).is_err());
    }

    /// A whole payment, offline: built from a wallet note, proven, signed and extracted (the extractor verifies the
    /// proof and the signatures). The statement's checks pass on it with the kept opening, the receiver's viewing key
    /// finds the same opening and reference, and another reference or opening is refused.
    #[test]
    fn a_payment_is_built_proven_and_checked() {
        let sk = SpendingKey::from_bytes(WALLET_KEY).unwrap();
        let fvk = FullViewingKey::from(&sk);
        let (note, anchor) = wallet_note(&fvk, 1_000_000);
        let recipient = Address::from_raw_address_bytes(&fixture_receiver()).unwrap();
        let payment_ref = fixture_payment_ref();
        // Testnet, in NU7's epoch: a v6 transaction.
        let target = BlockHeight::from_u32(4_500_001);
        let unsigned =
            build_payment(&Params::TestNetwork, target, &fvk, anchor, &[&note], recipient, 250_000, &payment_ref)
                .unwrap();
        assert_eq!((unsigned.fee, unsigned.change), (10_000, 740_000));
        let opening = unsigned.opening.clone();
        assert_eq!((opening.receiver, opening.value, opening.txid.clone()), (fixture_receiver(), 250_000, None));

        let tx = authorize(unsigned, &sk, &proving_key()).unwrap();
        check_payment(&TESTNET, &tx, &opening, &payment_ref).unwrap();
        assert_eq!(u32::from(tx.expiry_height()), 4_500_001 + EXPIRY_DELTA);
        assert!(!serialize(&tx).unwrap().is_empty());

        let receiver = FullViewingKey::from(&SpendingKey::from_bytes(FIXTURE_RECEIVER_SPENDING_KEY).unwrap());
        let found = find_payments(&tx, &receiver).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].opening.clone(), found[0].payment_ref), (opening.clone(), payment_ref));
        // The wallet's own change is not a payment: its memo names nothing.
        assert!(find_payments(&tx, &fvk).unwrap().is_empty());

        assert!(check_payment(&TESTNET, &tx, &opening, &[0; 32]).is_err());
        let mut wrong = opening.clone();
        wrong.value += 1;
        assert!(check_payment(&TESTNET, &tx, &wrong, &payment_ref).is_err());
        wrong = opening;
        wrong.action_index ^= 1;
        assert!(check_payment(&TESTNET, &tx, &wrong, &payment_ref).is_err());
    }
}
