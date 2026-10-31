//! `seam`: the memo, the witness and the proof of a shielded Ironwood payment for a Seam payment verifier
//! (docs/PAYMENTS.md).
//!
//! ```text
//! seam memo <paymentRef> [--address <ua> --amount <ZEC>]     the memo text, and a ZIP 321 payment URI
//! seam commitment --address <ua> --salt-file <file> [--new-salt]
//!                                                              the receiver commitment an app commits to
//! seam witness --txid <id> (--opening <file> | --ufvk-file <file>) --salt-file <file> --anchor <height>
//!              --out <witness.json>                            a witness, checked with the statement
//! seam pay --wallet <file> --to <ua> --zat <n> --ref <paymentRef> --opening <out.json>
//!                                                              a Testnet payment from a disposable wallet
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

mod commitment;
mod memo;
mod pay;
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
    /// Prints the receiver commitment keccak256(salt32 || receiver43) an app commits to, from the receiver's unified
    /// address and a salt file, which --new-salt creates.
    Commitment(commitment::CommitmentArgs),
    /// Builds the witness of a payment from chain data and its opening (the payer's, or found with the receiver's
    /// viewing key), and checks it with the statement.
    Witness(witness::WitnessArgs),
    /// Pays from a disposable Testnet wallet: one Ironwood note with the memo of a paymentRef and `ovk = None`, its
    /// opening and txid kept in a file. Creates the wallet with --new-wallet.
    Pay(pay::PayArgs),
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

/// Writes a new file only its owner may read (on Unix; elsewhere the directory's permissions apply). An existing file
/// or link at `path` is never replaced: one output can't overwrite another, or a wallet's key.
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options
        .open(path)
        .and_then(|mut f| f.write_all(bytes))
        .with_context(|| format!("write {} (a new file: an existing one is never replaced)", path.display()))
}

/// Refuses, before any work, the outputs `write_private` would refuse at the end: a path that exists (a file or a
/// link) or one named twice. A command then never stops half done, such as a new wallet without its address.
pub(crate) fn ensure_new(paths: &[&Path]) -> Result<()> {
    for (i, path) in paths.iter().enumerate() {
        ensure!(
            std::fs::symlink_metadata(path).is_err(),
            "{} exists: an existing file is never replaced",
            path.display()
        );
        ensure!(!paths[..i].contains(path), "{} is named for two outputs", path.display());
    }
    Ok(())
}

/// Warns when the light wallet server is reached without TLS, unless on this machine. Never prints the URL, which may
/// carry an API key.
pub(crate) fn warn_if_plaintext(url: &str) {
    if plaintext_remote(url) {
        eprintln!("warning: the light wallet server is reached without TLS: the network in between sees every request");
    }
}

/// Whether `url` is plain `http` to a host other than this machine (`localhost`, `127.0.0.1`, `[::1]`).
fn plaintext_remote(url: &str) -> bool {
    let url = url.to_ascii_lowercase();
    let Some(rest) = url.strip_prefix("http://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let host = match host.strip_prefix('[') {
        Some(v6) => v6.split_once(']').map_or(v6, |(host, _)| host),
        None => host.split_once(':').map_or(host, |(host, _)| host),
    };
    !matches!(host, "localhost" | "127.0.0.1" | "::1")
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Memo(args) => memo::run(args),
        Command::Commitment(args) => commitment::run(args),
        Command::Witness(args) => tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .context("start an async runtime")?
            .block_on(witness::run(args)),
        Command::Pay(args) => pay::run(args),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plaintext_remote_is_http_to_another_machine() {
        for url in [
            "http://lwd.example.com:9067",
            "HTTP://lwd.example.com",
            "http://localhost.example.com:9067",
            "http://127.0.0.1.example.com",
            "http://localhost@lwd.example.com",
            "http://[::2]:9067",
        ] {
            assert!(plaintext_remote(url), "{url}");
        }
        for url in [
            "https://lwd.example.com:443",
            "http://localhost:9067",
            "HTTP://LocalHost",
            "http://127.0.0.1:9067/",
            "http://[::1]:9067",
            "http://user:key@localhost:9067",
        ] {
            assert!(!plaintext_remote(url), "{url}");
        }
    }

    #[test]
    fn ensure_new_refuses_existing_and_repeated_outputs() {
        let dir = std::env::temp_dir().join(format!("seam-ensure-new-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (taken, free) = (dir.join("taken"), dir.join("free"));
        std::fs::write(&taken, b"").unwrap();
        assert!(ensure_new(&[free.as_path()]).is_ok());
        assert!(ensure_new(&[free.as_path(), taken.as_path()]).is_err());
        assert!(ensure_new(&[free.as_path(), free.as_path()]).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
