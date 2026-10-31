//! The Seam payment guest, embedded (ELF and image id) by risc0-build, for one Zcash network: Testnet, or Mainnet with
//! the `mainnet` feature.

/// The network the embedded guest proves on (`test` or `main`, as `seam_payment::Network::name`): its image id is
/// `payment-<network>` in zkvm/pinned-image-ids.txt.
pub const GUEST_NETWORK: &str = if cfg!(feature = "mainnet") { "main" } else { "test" };

include!(concat!(env!("OUT_DIR"), "/methods.rs"));
