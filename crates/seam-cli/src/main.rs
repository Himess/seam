//! `seam`: the memo, the witness and the proof of a shielded Ironwood payment for a Seam payment verifier
//! (docs/PAYMENTS.md).
//!
//! ```text
//! seam memo <paymentRef> [--address <ua> --amount <ZEC>]     the memo text, and a ZIP 321 payment URI
//! seam witness --txid <id> (--opening <file> | --ufvk-file <file>) --salt-file <file> --anchor <height>
//!              --out <witness.json>                            a witness, checked with the statement
//! seam execute | prove | groth16 | seal | image-id | export-elf | parity
//!                                                              with the `zkvm` feature: the RISC Zero guest
//! ```
//!
//! Secrets (an opening, a viewing key, a salt) come from files, never from the command line. `seam` prints counts,
//! heights and hashes of public data only: never a receiver, a salt, a key, a viewing key or a peer's address (only
//! `memo --address` prints an address: the one it is given, in the URI it was asked for). One build proves on one
//! Zcash network: Testnet, or Mainnet with the `mainnet` feature.

use std::{fs::OpenOptions, io::Write, path::Path, process::ExitCode};

use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand};
use seam_chain::ZcashNetwork;

mod memo;
mod witness;
#[cfg(feature = "zkvm")]
mod zkvm;

#[derive(Parser)]
#[command(
    name = "seam",
    version,
    about = "Shielded Ironwood payment proofs for EVM chains: memos, witnesses, proofs"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Prints the memo text that binds a payment to a paymentRef and, with --address and --amount, a ZIP 321 URI.
    Memo(memo::MemoArgs),
    /// Builds the witness of a payment from chain data and its opening (the payer's, or found with the receiver's
    /// viewing key), and checks it with the statement.
    Witness(witness::WitnessArgs),
    #[cfg(feature = "zkvm")]
    #[command(flatten)]
    Zkvm(zkvm::ZkvmCommand),
}

/// The network a command works on: this build's, which `--network` may only confirm.
pub(crate) fn network(arg: Option<&str>) -> Result<ZcashNetwork> {
    let build = ZcashNetwork::of_build();
    if let Some(name) = arg {
        let asked = ZcashNetwork::from_name(name)?;
        ensure!(
            asked == build,
            "this build proves payments on Zcash {build}: --network {asked} needs a build {} the `mainnet` feature",
            if asked == ZcashNetwork::Mainnet {
                "with"
            } else {
                "without"
            }
        );
    }
    Ok(build)
}

/// The light wallet server used unless `--lwd` names another (Zecured's, docs/ref/FACTS.md there).
pub(crate) fn default_lwd(network: ZcashNetwork) -> &'static str {
    match network {
        ZcashNetwork::Testnet => "https://testnet.zec.rocks:443",
        ZcashNetwork::Mainnet => "https://zec.rocks:443",
    }
}

/// The text of a file that holds a secret. Its path may be named in an error; its contents never are.
pub(crate) fn read_secret_text(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))
}

/// Writes a file only its owner may read (on Unix; elsewhere the directory's permissions apply).
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options
        .open(path)
        .and_then(|mut f| f.write_all(bytes))
        .with_context(|| format!("write {}", path.display()))
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Memo(args) => memo::run(args),
        Command::Witness(args) => tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .context("start an async runtime")?
            .block_on(witness::run(args)),
        // Outside any async runtime: RISC Zero's provers may start their own.
        #[cfg(feature = "zkvm")]
        Command::Zkvm(command) => zkvm::run(command),
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    // Note decryption, the statement and the zkVM host run on a thread with a larger stack than Windows gives the
    // main thread (1 MiB).
    let outcome = std::thread::Builder::new().stack_size(16 << 20).spawn(move || run(cli)).map(|t| t.join());
    match outcome {
        Ok(Ok(Ok(()))) => ExitCode::SUCCESS,
        Ok(Ok(Err(e))) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
        Ok(Err(_)) => ExitCode::FAILURE, // the thread panicked, and printed why
        Err(e) => {
            eprintln!("error: start a thread: {e}");
            ExitCode::FAILURE
        }
    }
}
