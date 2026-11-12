//! `seam mask`: the masked amount of an amount-hidden payment (`seam_payment::masked`, docs/PAYMENTS.md). An app that
//! keeps the amount encrypted publishes `M = Z + R (mod 2^64)` instead of the amount Z, where R is derived from the
//! receiver commitment's salt and the paymentRef. Before paying, the payer checks that a published M masks the amount it
//! agreed to; whoever proves needs M for the proof's input.
//!
//! It never prints R: R and M together reveal the amount.

use std::path::PathBuf;

use anyhow::{Context, Result, bail, ensure};
use clap::Args;
use seam_payment::masked::{masked_amount, masked_threshold};

use crate::{memo::parse_ref, read_secret_text};

#[derive(Args)]
pub struct MaskArgs {
    /// The receiver commitment's salt file (64 hex digits, as `seam commitment --new-salt` writes it).
    #[arg(long)]
    salt_file: PathBuf,
    /// The paymentRef the memo names (64 hex digits, with or without 0x).
    #[arg(long = "ref")]
    payment_ref: String,
    /// The amount Z, in zatoshi. Alone: prints M = Z + R. With --masked: checks that the published M masks it.
    #[arg(long)]
    zat: Option<u64>,
    /// A published M. Alone: prints the least value a payment must carry to prove against it (M - R).
    #[arg(long)]
    masked: Option<u64>,
}

pub fn run(args: MaskArgs) -> Result<()> {
    let salt = parse_ref(&read_secret_text(&args.salt_file)?).context("the salt file: 64 hex digits")?;
    let payment_ref = parse_ref(&args.payment_ref).context("--ref")?;
    match (args.zat, args.masked) {
        (Some(z), None) => println!("{}", masked_amount(z, &salt, &payment_ref)),
        (Some(z), Some(m)) => {
            ensure!(
                masked_threshold(m, &salt, &payment_ref) == z,
                "the published M does not mask {z} zatoshi with this salt and reference: don't pay"
            );
            println!("ok: M masks {z} zatoshi with this salt and reference");
        }
        (None, Some(m)) => println!("{}", masked_threshold(m, &salt, &payment_ref)),
        (None, None) => bail!("give --zat, --masked, or both"),
    }
    Ok(())
}
