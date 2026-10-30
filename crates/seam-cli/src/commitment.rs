//! `seam commitment`: the receiver commitment `keccak256(salt32 || receiver43)` an app commits to when it asks for a
//! payment (docs/PAYMENTS.md § 2), from the receiver's unified address and a salt. With `--new-salt` the salt is made
//! here: 32 random bytes, fresh for this payment, in a file only its owner may read. The salt stays off chain, with
//! the receiver and whoever proves the payment.

use std::path::PathBuf;

use anyhow::{Context, Result, ensure};
use clap::Args;
use rand_core::{OsRng, RngCore};
use seam_chain::ZcashNetwork;
use seam_payment::receiver_commitment;

use crate::{
    memo::{ironwood_receiver, parse_ref},
    read_secret_text, write_private,
};

#[derive(Args)]
pub struct CommitmentArgs {
    /// The receiver's unified address: an Ironwood payment pays its Orchard-protocol receiver.
    #[arg(long)]
    address: String,
    /// A file with the salt, as 64 hex digits; with --new-salt, the file to create.
    #[arg(long)]
    salt_file: PathBuf,
    /// Creates the salt file with 32 fresh random bytes. An existing file is never replaced.
    #[arg(long)]
    new_salt: bool,
    /// The address's Zcash network: this build's (testnet, or mainnet with the `mainnet` feature).
    #[arg(long, value_parser = ["testnet", "mainnet"])]
    network: Option<String>,
}

/// The receiver commitment of unified address `address` of Zcash `network` under `salt`, and whether the address has
/// receivers besides its Orchard-protocol one.
pub fn commitment(network: ZcashNetwork, address: &str, salt: &[u8; 32]) -> Result<([u8; 32], bool)> {
    let (receiver, other_receivers) = ironwood_receiver(network, address)?;
    Ok((receiver_commitment(salt, &receiver.to_raw_address_bytes()), other_receivers))
}

pub fn run(args: CommitmentArgs) -> Result<()> {
    let network = crate::network(args.network.as_deref())?;
    // The address is checked before a salt file is written.
    ironwood_receiver(network, &args.address).context("--address")?;
    let salt = if args.new_salt {
        ensure!(!args.salt_file.exists(), "{} exists: a salt file is never replaced", args.salt_file.display());
        let mut salt = [0u8; 32];
        OsRng.fill_bytes(&mut salt);
        write_private(&args.salt_file, format!("{}\n", hex::encode(salt)).as_bytes())?;
        salt
    } else {
        parse_ref(&read_secret_text(&args.salt_file)?).context("the salt file: 64 hex digits")?
    };
    let (c, other_receivers) = commitment(network, &args.address, &salt)?;
    println!("0x{}", hex::encode(c));
    if other_receivers {
        eprintln!(
            "note: the address also has a transparent or Sapling receiver; the commitment is to its Orchard-protocol \
             receiver, which an Ironwood payment pays"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use orchard::keys::{FullViewingKey, Scope, SpendingKey};
    use seam_payment::k4::{FIXTURE_RECEIVER_SPENDING_KEY, FIXTURE_SALT, fixture_receiver};
    use zcash_keys::address::UnifiedAddress;

    use super::*;

    /// The commitment of the fixture's receiver, given by its unified address, is the one the statement puts in the
    /// fixture's journal; the address must be of this network.
    #[test]
    fn the_commitment_is_the_journals() {
        let fvk = FullViewingKey::from(&SpendingKey::from_bytes(FIXTURE_RECEIVER_SPENDING_KEY).unwrap());
        let ua = UnifiedAddress::from_receivers(Some(fvk.address_at(0u32, Scope::External)), None, None, None, None)
            .unwrap();
        let address = ua.encode(&ZcashNetwork::Testnet.params());
        let (c, others) = commitment(ZcashNetwork::Testnet, &address, &FIXTURE_SALT).unwrap();
        assert_eq!(c, receiver_commitment(&FIXTURE_SALT, &fixture_receiver()));
        assert!(!others);
        assert_ne!(commitment(ZcashNetwork::Testnet, &address, &[0; 32]).unwrap().0, c);
        assert!(commitment(ZcashNetwork::Mainnet, &address, &FIXTURE_SALT).is_err());
    }
}
