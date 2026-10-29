//! Zcash chain data for Seam payment proofs (docs/PAYMENTS.md): what a `seam_payment::PaymentWitness` needs besides
//! the note's opening, rebuilt from public data.
//!
//! A payment proof shows that a transaction is in a block B whose ZIP 221 history-tree leaf a later header T commits
//! to through `hashBlockCommitments` (ZIP 244), without saying which block B is. This crate rebuilds that tree from
//! public chain data and checks it against the real headers. Ported from Zecured's witness builder
//! (`signer/crates/deposit-witness` at 73de9f909e270e8aea10c2b221905f62a6a4c4df), which built payment witnesses from
//! real Testnet data on 27 October 2026.

pub mod history;
pub mod mmr;
pub mod network;

pub use network::{Epoch, ZcashNetwork};
