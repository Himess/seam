//! The Seam payment guest: proves that the private witness satisfies the payment statement (`seam-payment`) and
//! commits its public output, ABI-encoded for `ZcashPaymentVerifier.verify`, as the journal.

use risc0_zkvm::guest::env;
use seam_payment::{PaymentWitness, verify};

fn main() {
    // One bincode frame: its length, then its bytes.
    let mut len = 0u32;
    env::read_slice(core::slice::from_mut(&mut len));
    let mut frame = vec![0u8; len as usize];
    env::read_slice(&mut frame);
    let witness: PaymentWitness = bincode::deserialize(&frame).expect("witness encoding");
    match verify(&witness) {
        Ok(journal) => env::commit_slice(&journal.abi_encode()),
        Err(e) => panic!("not a valid payment: {e:?}"),
    }
}
