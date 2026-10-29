//! Zcash chain data for Seam payment proofs (docs/PAYMENTS.md): what a `seam_payment::PaymentWitness` needs besides
//! the note's opening, rebuilt from public data.
//!
//! A payment proof shows that a transaction is in a block B whose ZIP 221 history-tree leaf a later header T commits
//! to through `hashBlockCommitments` (ZIP 244), without saying which block B is. This crate rebuilds that tree from
//! public chain data and checks it against the real headers. Ported from Zecured's witness builder
//! (`signer/crates/deposit-witness` at 73de9f909e270e8aea10c2b221905f62a6a4c4df), which built payment witnesses from
//! real Testnet data on 27 October 2026.
//!
//! - [`p2p`] and [`headers`]: the epoch's headers from the peer-to-peer network, in a checked header file;
//! - [`lwd`] and [`chain`]: compact blocks and tree states from a light wallet server, and the per-block summaries a
//!   history-tree leaf records, in a checked cache;
//! - [`history`] and [`mmr`]: the history tree, a leaf's path and the peaks, and a block's txid and auth data trees;
//! - [`locate`]: a transaction's place under an anchor header T;
//! - [`witness`]: the payment witness, from the payer's opening or the receiver's viewing key;
//! - [`scan`] and [`pay`]: a disposable wallet's notes, and a payment from it with the Seam memo and `ovk = None`.
//!
//! Nothing here prints. Errors name heights and counts, never a receiver, a salt, a key or a peer's address, and
//! never the height of the block that holds a payment.

pub mod chain;
pub mod headers;
pub mod history;
pub mod locate;
pub mod lwd;
pub mod mmr;
pub mod network;
pub mod p2p;
pub mod pay;
pub mod scan;
pub mod witness;

pub use network::{Epoch, ZcashNetwork};
