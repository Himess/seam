//! The amount-hidden payment statement (journal version 2): it accepts exactly the payments of version 1 whose value
//! is at least `M - R`, and its journal is version 1's with the amount replaced by M.
//!
//! The vectors for R were computed independently, with ethers' keccak256, so the Rust, Solidity and TypeScript sides
//! agree on the mask's definition.

use std::fs;

use seam_payment::masked::{
    MASKED_JOURNAL_VERSION, MaskedError, MaskedPaymentInput, amount_mask, masked_amount, masked_threshold,
    verify_masked_on,
};
use seam_payment::{JOURNAL_LENGTH, MAINNET, PaymentWitness, TESTNET, receiver_commitment, verify_on};

/// The real Testnet payment's witness (TEST-ONLY: two disposable wallets), 0.01 TAZ = 1,000,000 zatoshi.
const REAL_WITNESS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/TEST-ONLY_payment_witness.json");
const REAL_VALUE: u64 = 1_000_000;
/// R for the real witness's salt and paymentRef (computed with ethers).
const REAL_MASK: u64 = 0x2ff4_442c_bb39_1e3b;

fn real() -> PaymentWitness {
    serde_json::from_slice(&fs::read(REAL_WITNESS).expect("fixture")).expect("a witness")
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[test]
fn the_mask_matches_the_independent_vectors() {
    assert_eq!(amount_mask(&[0u8; 32], &[0u8; 32]), 0x0cd0_4af7_2b7b_9602);
    assert_eq!(amount_mask(&[0xffu8; 32], &[0u8; 32]), 0xdfa0_8b7c_9f65_6d57);
    let w = real();
    let j = verify_on(&TESTNET, &w).expect("the real payment");
    assert_eq!(amount_mask(&w.salt, &j.payment_ref), REAL_MASK);
}

#[test]
fn masking_wraps_mod_2_64_and_unmasking_inverts_it() {
    let (salt, r) = ([7u8; 32], [9u8; 32]);
    for z in [0u64, 1, REAL_VALUE, u64::MAX - 1, u64::MAX] {
        assert_eq!(masked_threshold(masked_amount(z, &salt, &r), &salt, &r), z);
    }
}

/// M = value + R proves, and the journal is version 1's with version 2 and the amount's word replaced by M.
#[test]
fn the_real_payment_proves_against_its_exact_masked_amount() {
    let w = real();
    let v1 = verify_on(&TESTNET, &w).expect("version 1");
    let m = REAL_VALUE.wrapping_add(REAL_MASK);
    let j = verify_masked_on(
        &TESTNET,
        &MaskedPaymentInput {
            witness: w.clone(),
            masked_amount: m,
        },
    )
    .expect("masked");
    assert_eq!(j.version, MASKED_JOURNAL_VERSION);
    assert_eq!(
        (j.branch_id, j.anchor, j.payment_ref, j.receiver_commitment),
        (v1.branch_id, v1.anchor, v1.payment_ref, v1.receiver_commitment)
    );
    assert_eq!(j.masked_amount, m);

    let enc = j.abi_encode();
    assert_eq!(enc.len(), JOURNAL_LENGTH);
    let v1enc = v1.abi_encode();
    assert_eq!(hex(&enc[..32]), format!("{:064x}", 2));
    assert_eq!(enc[32..160], v1enc[32..160], "branch, anchor, paymentRef and commitment unchanged");
    assert_eq!(hex(&enc[160..]), format!("{m:064x}"));
    // the amount appears nowhere in the journal
    assert!(!hex(&enc).contains(&format!("{REAL_VALUE:016x}")));

    let mainnet = verify_masked_on(
        &MAINNET,
        &MaskedPaymentInput {
            witness: w,
            masked_amount: m,
        },
    );
    assert_eq!(mainnet, Ok(j));
}

#[test]
fn any_threshold_at_or_below_the_value_proves_and_above_it_does_not() {
    let w = real();
    for z in [0u64, 1, REAL_VALUE / 2, REAL_VALUE - 1, REAL_VALUE] {
        let input = MaskedPaymentInput {
            witness: w.clone(),
            masked_amount: z.wrapping_add(REAL_MASK),
        };
        assert!(verify_masked_on(&TESTNET, &input).is_ok(), "threshold {z}");
    }
    for z in [REAL_VALUE + 1, REAL_VALUE * 2, u64::MAX] {
        let input = MaskedPaymentInput {
            witness: w.clone(),
            masked_amount: z.wrapping_add(REAL_MASK),
        };
        assert_eq!(verify_masked_on(&TESTNET, &input), Err(MaskedError::Underpaid), "threshold {z}");
    }
}

/// A mask that doesn't match the salt shifts the threshold: M - R wraps to a huge value just below R.
#[test]
fn a_masked_amount_built_without_the_right_mask_does_not_prove() {
    let w = real();
    // M = R - 1, i.e. a threshold of 2^64 - 1
    let input = MaskedPaymentInput {
        witness: w.clone(),
        masked_amount: REAL_MASK.wrapping_sub(1),
    };
    assert_eq!(verify_masked_on(&TESTNET, &input), Err(MaskedError::Underpaid));
    // M = value + a different mask, here R(salt = 0, same reference)
    let j = verify_on(&TESTNET, &w).unwrap();
    let wrong = REAL_VALUE.wrapping_add(amount_mask(&[0u8; 32], &j.payment_ref));
    let res = verify_masked_on(
        &TESTNET,
        &MaskedPaymentInput {
            witness: w,
            masked_amount: wrong,
        },
    );
    assert!(res.is_err() || res.unwrap().masked_amount != REAL_VALUE.wrapping_add(REAL_MASK));
}

/// Another salt is accepted by the statement (as in version 1), but it changes both the receiver commitment in the
/// journal and the mask: the app's commitment check refuses it.
#[test]
fn another_salt_changes_the_commitment_and_the_mask() {
    let mut w = real();
    let j1 = verify_on(&TESTNET, &w).unwrap();
    let receiver: [u8; 43] = w.note_recipient.as_slice().try_into().unwrap();
    w.salt = [0x42; 32];
    let j2 = verify_on(&TESTNET, &w).unwrap();
    assert_ne!(j2.receiver_commitment, j1.receiver_commitment);
    assert_eq!(j2.receiver_commitment, receiver_commitment(&w.salt, &receiver));
    assert_ne!(amount_mask(&w.salt, &j2.payment_ref), REAL_MASK);
}

#[test]
fn a_witness_that_fails_version_1_fails_version_2_the_same_way() {
    let mut w = real();
    w.branch_id = 0x1234_5678;
    let v1 = verify_on(&TESTNET, &w).unwrap_err();
    let input = MaskedPaymentInput {
        witness: w,
        masked_amount: 0,
    };
    assert_eq!(verify_masked_on(&TESTNET, &input), Err(MaskedError::Payment(v1)));
}

#[test]
fn a_debug_print_shows_only_the_masked_amount() {
    let input = MaskedPaymentInput {
        witness: real(),
        masked_amount: 5,
    };
    assert_eq!(format!("{input:?}"), "MaskedPaymentInput { masked_amount: 5, .. }");
}
