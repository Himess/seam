//! The Seam amount-hidden payment guest: proves that the private input satisfies the amount-hidden payment statement
//! (`seam_payment::masked`: the payment statement, and a value of at least `M - R`) and commits its public output,
//! ABI-encoded for `ZcashMaskedPaymentVerifier.verify`, as the journal (version 2: M in place of the amount).

use risc0_zkvm::guest::env;
use seam_payment::masked::{MaskedPaymentInput, verify_masked};

fn main() {
    // One bincode frame: its length, then its bytes.
    let mut len = 0u32;
    env::read_slice(core::slice::from_mut(&mut len));
    let mut frame = vec![0u8; len as usize];
    env::read_slice(&mut frame);
    let input: MaskedPaymentInput = bincode::deserialize(&frame).expect("input encoding");
    match verify_masked(&input) {
        Ok(journal) => env::commit_slice(&journal.abi_encode()),
        Err(e) => panic!("not a valid masked payment: {e:?}"),
    }
}
