//! K4 for the payment statement: every relation has a negative test.
//!
//! The fixture (`TEST-ONLY_testnet_ironwood_tx.json`) is public chain data: a real Ironwood-only v6 transaction mined
//! in Zcash Testnet block 4,488,543 and its real inclusion data under header 4,488,560. These tests check that it is,
//! then build the synthetic payment witness from it (`k4::synthesize`) and run the catalog of single changes on it for
//! both networks. `seam parity` runs the same catalog in the RISC Zero executor. `TEST-ONLY_payment_witness.json` is a
//! real payment's witness, between two disposable Testnet wallets.

use std::fs;

use orchard::{
    keys::{FullViewingKey, Scope, SpendingKey},
    note_encryption::IronwoodDomain,
};
use seam_payment::k4::{
    FIXTURE_RECEIVER_SPENDING_KEY, FIXTURE_RSEED, FIXTURE_SALT, FIXTURE_VALUE, PublicTx, as_expected,
    fixture_payment_ref, fixture_receiver, mutations, synthesize, with_opening,
};
use seam_payment::{
    Error, Inclusion, JOURNAL_LENGTH, JOURNAL_VERSION, MAINNET, NU7_BRANCH_ID, PaymentWitness, TESTNET,
    check_inclusion, check_payment_transaction_on, memo_payment_ref, receiver_commitment, sha256d, verify_on,
};
use zcash_note_encryption::try_note_decryption;
use zcash_primitives::transaction::Transaction;
use zcash_protocol::consensus::BranchId;

const PUBLIC_TX: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/TEST-ONLY_testnet_ironwood_tx.json");
/// The relay's fixture whose first header is T, the public transaction's anchor.
const ANCHOR_FIXTURE: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../contracts/test/relay/fixtures/testnet_deposit_anchor.json");
/// The real Testnet payment's witness (TEST-ONLY: two disposable wallets).
const REAL_WITNESS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/TEST-ONLY_payment_witness.json");

fn public_tx() -> PublicTx {
    serde_json::from_slice(&fs::read(PUBLIC_TX).expect("fixture")).expect("public transaction JSON")
}

fn base() -> PaymentWitness {
    synthesize(&public_tx())
}

fn display(hash: &[u8; 32]) -> String {
    hash.iter().rev().map(|b| format!("{b:02x}")).collect()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn fixture_source(key: &str) -> serde_json::Value {
    let json: serde_json::Value = serde_json::from_slice(&fs::read(PUBLIC_TX).expect("fixture")).expect("JSON");
    json["source"][key].clone()
}

/// The fixture is what it says: a v6 Ironwood-only Testnet transaction whose txid is in block 4,488,543, a leaf of the
/// NU7 chain history committed to by header 4,488,560, which is also the first header of the relay's
/// `testnet_deposit_anchor.json`.
#[test]
fn the_public_transaction_is_real_chain_data() {
    let p = public_tx();
    let tx = Transaction::read(&p.tx[..], BranchId::Nu7).expect("a v6 transaction");
    check_payment_transaction_on(&TESTNET, &tx).expect("Ironwood-only, outputs enabled");
    let included = check_inclusion(
        tx.txid().as_ref(),
        &Inclusion {
            branch_id: p.branch_id,
            tx_index: p.tx_index,
            txid_path: &p.txid_path,
            block_header: &p.block_header,
            leaf: &p.leaf,
            mmr_path: &p.mmr_path,
            peaks: &p.peaks,
            peak_index: p.peak_index,
            tip_header: &p.tip_header,
            tip_auth_data_root: &p.tip_auth_data_root,
        },
    )
    .expect("the real inclusion data");
    assert_eq!(p.branch_id, NU7_BRANCH_ID);
    assert_eq!(included.block_height, 4_488_543);
    assert_eq!(fixture_source("block_height"), 4_488_543);
    assert_eq!(display(&sha256d(&p.block_header)), fixture_source("block_hash_display"));
    assert_eq!(display(&included.tip_hash), fixture_source("tip_hash_display"));

    let anchors: serde_json::Value =
        serde_json::from_slice(&fs::read(ANCHOR_FIXTURE).expect("relay fixture")).expect("JSON");
    assert_eq!(anchors["header_heights"][0], 4_488_560);
    assert_eq!(anchors["header_hashes_display"][0], display(&included.tip_hash));
}

/// `paymentRef = keccak256(abi.encode(uint256 84532, address 0x1111…11, uint256 7))`, the vector Foundry's
/// `cast keccak $(cast abi-encode "f(uint256,address,uint256)" 84532 0x1111111111111111111111111111111111111111 7)`
/// gives.
#[test]
fn the_fixture_reference() {
    assert_eq!(hex(&fixture_payment_ref()), "0ffd65bace87c3ecbd455fa36283257624b7a3dcdfaa610f6f37d30dd1cbbf19");
}

#[test]
fn the_synthetic_payment_proves() {
    let w = base();
    let j = verify_on(&TESTNET, &w).expect("the synthetic payment");
    assert_eq!(j.version, JOURNAL_VERSION);
    assert_eq!(j.branch_id, NU7_BRANCH_ID);
    assert_eq!(j.payment_ref, fixture_payment_ref());
    assert_eq!(j.receiver_commitment, receiver_commitment(&FIXTURE_SALT, &fixture_receiver()));
    assert_eq!(j.zatoshi, FIXTURE_VALUE);
    assert_eq!(j.anchor, sha256d(&w.tip_header), "T' is the anchor");
    assert_ne!(j.anchor, sha256d(&public_tx().tip_header), "re-anchored: T' is in no chain");
    assert_eq!(j.abi_encode().len(), JOURNAL_LENGTH);
    // The Mainnet build allows NU7 too: what separates the networks is the relay the anchor must be on.
    assert_eq!(verify_on(&MAINNET, &w), Ok(j));
}

/// A witness's debug print shows its epoch alone: no transaction, receiver, rseed or salt.
#[test]
fn a_witness_debug_print_shows_the_epoch_alone() {
    assert_eq!(format!("{:?}", base()), format!("PaymentWitness {{ branch_id: {NU7_BRANCH_ID}, .. }}"));
}

/// A real payment: 0.01 TAZ between two disposable Testnet wallets on 30 October 2026, with the memo of the fork test's
/// order (contracts/test/examples/ShieldedPaymentEscrowFork.t.sol), anchored at block 4,557,390, which was fixed before
/// the payment was made. `seam witness` built this witness from the payer's opening and, the same bytes, from the
/// receiver's viewing key. It proves on both networks' statements, to the journal the guest must commit.
#[test]
fn the_real_testnet_payment_proves() {
    let w: PaymentWitness = serde_json::from_slice(&fs::read(REAL_WITNESS).expect("fixture")).expect("a witness");
    let j = verify_on(&TESTNET, &w).expect("the real payment");
    assert_eq!(j.branch_id, NU7_BRANCH_ID);
    assert_eq!(display(&j.anchor), "000000c287c46c1f714dfcee970a4654300a099902b26e03975d817846ed3dcb");
    assert_eq!(hex(&j.payment_ref), "39458b5c2bea9b8e50aa1e413970a4710aa096b57bd98c977fb2d060f973cb9f");
    assert_eq!(hex(&j.receiver_commitment), "94a35f10c1b6b65cbf8962a7bd68ee1e4920841b8d529a41b71b036335a08245");
    assert_eq!(j.zatoshi, 1_000_000);
    assert_eq!(
        hex(&j.abi_encode()),
        concat!(
            "0000000000000000000000000000000000000000000000000000000000000001",
            "0000000000000000000000000000000000000000000000000000000077190ad9",
            "cb3ded4678815d97036eb20299090a3054460a97eefc4d711f6cc487c2000000",
            "39458b5c2bea9b8e50aa1e413970a4710aa096b57bd98c977fb2d060f973cb9f",
            "94a35f10c1b6b65cbf8962a7bd68ee1e4920841b8d529a41b71b036335a08245",
            "00000000000000000000000000000000000000000000000000000000000f4240",
        )
    );
    assert_eq!(verify_on(&MAINNET, &w), Ok(j));
}

/// The real transaction under its real anchor, with the fixture's opening: the action carries another note.
#[test]
fn the_unchanged_transaction_does_not_open_to_the_fixture_note() {
    let w = with_opening(&public_tx(), fixture_receiver(), FIXTURE_VALUE, FIXTURE_RSEED, FIXTURE_SALT);
    assert_eq!(verify_on(&TESTNET, &w), Err(Error::NotDecryptable));
}

/// The receiver finds the payment with its incoming viewing key by trial decryption, as `seam witness --ufvk-file`
/// does, and reads the same note and reference the payer's opening gives.
#[test]
fn the_receiver_finds_the_payment_with_its_viewing_key() {
    let w = base();
    let tx = Transaction::read(&w.tx[..], BranchId::Nu7).expect("a v6 transaction");
    let action = &tx.ironwood_bundle().expect("an Ironwood bundle").actions()[w.action_index as usize];
    let sk = SpendingKey::from_bytes(FIXTURE_RECEIVER_SPENDING_KEY).expect("a spending key");
    let ivk = FullViewingKey::from(&sk).to_ivk(Scope::External).prepare();
    let (note, to, memo) =
        try_note_decryption(&IronwoodDomain::for_action(action), &ivk, action).expect("the receiver's note");
    assert_eq!(to.to_raw_address_bytes(), fixture_receiver());
    assert_eq!(note.value().inner(), FIXTURE_VALUE);
    assert_eq!(*note.rseed().as_bytes(), FIXTURE_RSEED);
    assert_eq!(memo_payment_ref(&memo), Some(fixture_payment_ref()));
    // Another receiver's key finds nothing.
    let other = SpendingKey::from_bytes([0x08; 32]).expect("a spending key");
    let other_ivk = FullViewingKey::from(&other).to_ivk(Scope::External).prepare();
    assert!(try_note_decryption(&IronwoodDomain::for_action(action), &other_ivk, action).is_none());
}

/// Every single change gives exactly the outcome the catalog asks, on both networks' statements.
#[test]
fn the_k4_catalog() {
    let base = base();
    for net in [TESTNET, MAINNET] {
        let base_journal = verify_on(&net, &base).expect("the base witness proves");
        let catalog = mutations(&net);
        // Mainnet's statement also proves NU6.3, and has one more change.
        assert_eq!(catalog.len(), if net == MAINNET { 60 } else { 59 }, "changes on {}", net.name);
        for m in catalog {
            let mut w = base.clone();
            (m.apply)(&mut w);
            let got = verify_on(&net, &w);
            // An accepted witness's anchor is its own T, whatever the change.
            if let Ok(j) = &got {
                assert_eq!(j.anchor, sha256d(&w.tip_header), "{} on {}: the anchor is T's hash", m.name, net.name);
            }
            assert!(
                as_expected(m.expected, &base_journal, &got),
                "{} on {}: got {got:?}, expected {:?}",
                m.name,
                net.name,
                m.expected
            );
        }
    }
}
