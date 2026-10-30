//! The zkVM subcommands (cargo feature `zkvm`): RISC Zero's executor and prover on the Seam payment guest
//! (zkvm/guest, embedded by zkvm/methods). Ported from the payment parts of Zecured's `tier2/host/src/main.rs`
//! (73de9f909e270e8aea10c2b221905f62a6a4c4df), which proved a real payment on 27 October 2026.
//!
//! ```text
//! seam execute <witness.json>               the executor: cycles, and the journal against the native statement's
//! seam prove <witness.json> <receipt.bin>   a succinct (STARK) receipt (segments of 2^SEAM_SEGMENT_PO2 cycles)
//! seam groth16 <receipt.bin> <seal.json>    its Groth16 wrap (x86 with Docker), written as seal.json
//! seam seal <witness.json> <seal.json>      prove, then groth16, in one run
//! seam image-id                             the guest's image id
//! seam export-elf <guest.bin>               the guest, to pin it
//! seam parity <public-tx.json>              K4: each change, the same outcome natively and in the executor
//! seam p254 | shrink-wrap                   with the `cuda` feature: the Groth16 wrap in two processes
//! ```
//!
//! A witness path may be `-`: the witness is then read from standard input. `seal.json` is `{imageId, journal,
//! seal}`, what `ZcashPaymentVerifier.verify(journal, seal)` takes; the seal opens with the 4-byte selector the
//! RISC Zero router dispatches on.
//!
//! The guest's image id depends on where it was built, and a verifier pins one for good, so provers run the
//! reproducibly built guest: with `SEAM_PAYMENT_ELF=<guest.bin>` set, every command uses that file instead of the
//! guest embedded in this binary. One build proves on one network: the embedded guest and the native statement both
//! follow the `mainnet` feature.

use std::{
    io::Read,
    path::{Path, PathBuf},
    time::Instant,
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use clap::Subcommand;
use risc0_zkvm::{ExecutorEnv, ProverOpts, Receipt, compute_image_id, default_executor, default_prover, sha::Digest};
use seam_payment::{JOURNAL_LENGTH, NETWORK, PaymentWitness, k4, verify};
use seam_zkvm_methods::{GUEST_NETWORK, PAYMENT_ELF, PAYMENT_ID};

#[derive(Subcommand)]
pub enum ZkvmCommand {
    /// Runs the guest on a witness in RISC Zero's executor: its cycles, and its journal against the native statement's.
    Execute {
        /// The witness (JSON), or - for standard input.
        witness: String,
    },
    /// Proves the guest on a witness: a succinct (STARK) receipt, verified. Segments of 2^SEAM_SEGMENT_PO2 cycles: 18
    /// by default (about 3 GB of memory), 20 on a 24 GB GPU.
    Prove {
        /// The witness (JSON), or - for standard input.
        witness: String,
        receipt: PathBuf,
    },
    /// Wraps a succinct receipt in Groth16 (x86 with Docker, or natively with the `cuda` feature) and writes seal.json.
    Groth16 { receipt: PathBuf, seal: PathBuf },
    /// Proves a witness and wraps the receipt in Groth16 in one run: seal.json straight from a witness.
    Seal {
        /// The witness (JSON), or - for standard input.
        witness: String,
        seal: PathBuf,
    },
    /// Prints the guest's image id.
    ImageId,
    /// Writes the guest's ELF, to pin it.
    ExportElf { out: PathBuf },
    /// K4: every change of seam-payment's catalog has its outcome natively and the same outcome in the executor.
    Parity {
        /// A public transaction (crates/seam-payment/tests/fixtures/TEST-ONLY_testnet_ironwood_tx.json), or - for
        /// standard input: the base is the synthetic witness seam-payment builds from it, the one the catalog's
        /// outcomes are written for.
        public_tx: String,
    },
    /// The first half of groth16, in its own process: the receipt's Poseidon254 identity receipt.
    #[cfg(feature = "cuda")]
    P254 { receipt: PathBuf, out: PathBuf },
    /// The second half of groth16: shrinks and wraps the identity receipt in Groth16, and writes seal.json.
    #[cfg(feature = "cuda")]
    ShrinkWrap {
        p254: PathBuf,
        receipt: PathBuf,
        seal: PathBuf,
    },
}

/// The guest every command runs: the pinned file named by `SEAM_PAYMENT_ELF`, or the one embedded in this binary.
struct Guest {
    elf: Vec<u8>,
    id: Digest,
}

impl Guest {
    fn load() -> Result<Self> {
        match std::env::var_os("SEAM_PAYMENT_ELF") {
            Some(path) => {
                let elf = std::fs::read(&path).context("read SEAM_PAYMENT_ELF")?;
                let id = compute_image_id(&elf).context("SEAM_PAYMENT_ELF is not a guest program")?;
                Ok(Self { elf, id })
            }
            // A build without its guest (RISC0_SKIP_BUILD, where RISC Zero cannot build it) proves nothing and names no
            // image id until a guest is pinned.
            None if PAYMENT_ELF.is_empty() => {
                bail!("this build embeds no guest: set SEAM_PAYMENT_ELF to the pinned guest")
            }
            None => Ok(Self {
                elf: PAYMENT_ELF.to_vec(),
                id: Digest::from(PAYMENT_ID),
            }),
        }
    }

    fn image_id(&self) -> [u8; 32] {
        self.id.as_bytes().try_into().expect("32 bytes")
    }
}

/// The bytes of a witness file, or of standard input for `-` (a pipe, on platforms without `/dev/stdin`).
fn read_witness(path: &str) -> Result<Vec<u8>> {
    if path == "-" {
        let mut bytes = Vec::new();
        std::io::stdin().read_to_end(&mut bytes).context("read the witness from standard input")?;
        Ok(bytes)
    } else {
        std::fs::read(path).with_context(|| format!("read {path}"))
    }
}

/// The witness at `path`, checked natively: its bincode frame (the guest's input) and the journal the guest must
/// commit.
fn load_witness(path: &str) -> Result<(Vec<u8>, Vec<u8>)> {
    let witness: PaymentWitness = serde_json::from_slice(&read_witness(path)?).context("a payment witness (JSON)")?;
    let journal = verify(&witness)
        .map_err(|e| anyhow!("the statement refuses the witness natively: {e:?}"))?
        .abi_encode();
    Ok((bincode::serialize(&witness)?, journal))
}

fn env_for_frame(frame: &[u8], po2: Option<u32>) -> Result<ExecutorEnv<'static>> {
    let mut builder = ExecutorEnv::builder();
    builder.write_frame(frame);
    if let Some(po2) = po2 {
        builder.segment_limit_po2(po2);
    }
    builder.build()
}

/// Segments of 2^18 cycles keep memory around 3 GB on an 8 GB machine; a 24 GB GPU proves faster with larger ones
/// (`SEAM_SEGMENT_PO2=20`).
fn segment_po2() -> Result<u32> {
    match std::env::var("SEAM_SEGMENT_PO2") {
        Ok(v) => v.parse().context("SEAM_SEGMENT_PO2"),
        Err(_) => Ok(18),
    }
}

fn write_seal(guest: &Guest, out: &Path, journal: &[u8], seal: &[u8]) -> Result<()> {
    let json = serde_json::json!({
        "imageId": format!("0x{}", hex::encode(guest.image_id())),
        "journal": format!("0x{}", hex::encode(journal)),
        "seal": format!("0x{}", hex::encode(seal)),
    });
    std::fs::write(out, serde_json::to_vec_pretty(&json)?).with_context(|| format!("write {}", out.display()))
}

/// The succinct receipt of the guest on a witness's frame, verified, with the journal the native statement gives.
fn prove_succinct(guest: &Guest, frame: &[u8], journal: &[u8]) -> Result<Receipt> {
    let started = Instant::now();
    let info = default_prover().prove_with_opts(
        env_for_frame(frame, Some(segment_po2()?))?,
        &guest.elf,
        &ProverOpts::succinct(),
    )?;
    info.receipt.verify(guest.id)?;
    ensure!(info.receipt.journal.bytes == journal, "the receipt's journal is not the native statement's");
    println!(
        "succinct receipt for {} cycles ({} segments) in {:.1?}, verified",
        info.stats.total_cycles,
        info.stats.segments,
        started.elapsed()
    );
    Ok(info.receipt)
}

/// The Groth16 seal of a succinct receipt of the guest, verified, prefixed with the selector the router dispatches on:
/// the first four bytes of the verifier parameters digest.
fn groth16_seal(guest: &Guest, receipt: &Receipt) -> Result<Vec<u8>> {
    receipt.verify(guest.id).context("the receipt is not a proof of this guest")?;
    ensure!(receipt.journal.bytes.len() == JOURNAL_LENGTH, "the receipt's journal is not a payment journal");
    let started = Instant::now();
    let wrapped = default_prover().compress(&ProverOpts::groth16(), receipt)?;
    wrapped.verify(guest.id)?;
    let g = wrapped.inner.groth16()?;
    println!("Groth16 wrap in {:.1?}, verified", started.elapsed());
    Ok([&g.verifier_parameters.as_bytes()[..4], &g.seal[..]].concat())
}

/// K4: every change of seam-payment's catalog (for this build's network) of the synthetic witness built from the
/// public transaction at `input` has its outcome natively and the same outcome in the executor: the guest panics with
/// the statement's own error, or commits the same journal.
fn parity(guest: &Guest, input: &str) -> Result<()> {
    let public_tx =
        serde_json::from_slice::<k4::PublicTx>(&read_witness(input)?).context("a public transaction (JSON)")?;
    let base = k4::synthesize(&public_tx);
    let real = verify(&base).map_err(|e| anyhow!("the base witness does not prove: {e:?}"))?;
    let started = Instant::now();
    let session = default_executor().execute(env_for_frame(&bincode::serialize(&base)?, None)?, &guest.elf)?;
    ensure!(session.journal.bytes == real.abi_encode(), "the unchanged witness's journal differs in the zkVM");
    println!(
        "unchanged witness ({} network): proves, {} cycles in {:.1?}",
        NETWORK.name,
        session.cycles(),
        started.elapsed()
    );
    let mut failures = 0;
    for m in k4::mutations(&NETWORK) {
        let mut w = base.clone();
        (m.apply)(&mut w);
        let native = verify(&w);
        let started = Instant::now();
        let executed = default_executor().execute(env_for_frame(&bincode::serialize(&w)?, None)?, &guest.elf);
        let elapsed = started.elapsed();
        let as_expected = k4::as_expected(m.expected, &real, &native);
        let same_zkvm = match (&native, &executed) {
            (Err(e), Err(x)) => format!("{x:#}").contains(&format!("not a valid payment: {e:?}")),
            (Ok(j), Ok(session)) => session.journal.bytes == j.abi_encode(),
            _ => false,
        };
        let ok = |b: bool| if b { "ok" } else { "DIFFERS" };
        println!(
            "{:<66} {:<34} native {} zkVM {} ({elapsed:.1?})",
            m.name,
            m.expected.to_string(),
            ok(as_expected),
            ok(same_zkvm)
        );
        failures += usize::from(!(as_expected && same_zkvm));
    }
    ensure!(failures == 0, "{failures} changes did not have their outcome natively and the same in the zkVM");
    println!("every change has its outcome natively and the same in the zkVM");
    Ok(())
}

pub fn run(command: ZkvmCommand) -> Result<()> {
    // One switch sets both (the `mainnet` feature): a guest of one network with the native statement of another would
    // check witnesses natively for another network than the guest proves.
    ensure!(
        GUEST_NETWORK == NETWORK.name,
        "the embedded guest proves on {GUEST_NETWORK}, the native statement on {}: build with or without `mainnet`",
        NETWORK.name
    );
    let guest = Guest::load()?;
    match command {
        ZkvmCommand::ImageId => println!("0x{}", hex::encode(guest.image_id())),
        ZkvmCommand::ExportElf { out } => {
            std::fs::write(&out, &guest.elf).with_context(|| format!("write {}", out.display()))?;
            println!(
                "guest 0x{} ({} network) written to {}",
                hex::encode(guest.image_id()),
                NETWORK.name,
                out.display()
            );
        }
        ZkvmCommand::Execute { witness } => {
            println!("image id 0x{} ({} network)", hex::encode(guest.image_id()), NETWORK.name);
            let (frame, journal) = load_witness(&witness)?;
            let started = Instant::now();
            let session = default_executor().execute(env_for_frame(&frame, None)?, &guest.elf)?;
            ensure!(session.journal.bytes == journal, "the journal differs from the native statement's");
            println!(
                "executed in {:.1?}: {} cycles, {} segments; the journal is the native statement's",
                started.elapsed(),
                session.cycles(),
                session.segments.len()
            );
            println!("journal 0x{}", hex::encode(&journal));
        }
        ZkvmCommand::Prove { witness, receipt } => {
            println!("image id 0x{} ({} network)", hex::encode(guest.image_id()), NETWORK.name);
            let (frame, journal) = load_witness(&witness)?;
            let proven = prove_succinct(&guest, &frame, &journal)?;
            std::fs::write(&receipt, bincode::serialize(&proven)?)
                .with_context(|| format!("write {}", receipt.display()))?;
            println!("receipt written to {}", receipt.display());
        }
        ZkvmCommand::Groth16 { receipt, seal } => {
            println!("image id 0x{} ({} network)", hex::encode(guest.image_id()), NETWORK.name);
            let proven: Receipt =
                bincode::deserialize(&std::fs::read(&receipt).with_context(|| format!("read {}", receipt.display()))?)?;
            let bytes = groth16_seal(&guest, &proven)?;
            write_seal(&guest, &seal, &proven.journal.bytes, &bytes)?;
            println!("seal written to {}", seal.display());
        }
        ZkvmCommand::Seal { witness, seal } => {
            println!("image id 0x{} ({} network)", hex::encode(guest.image_id()), NETWORK.name);
            let (frame, journal) = load_witness(&witness)?;
            let proven = prove_succinct(&guest, &frame, &journal)?;
            let bytes = groth16_seal(&guest, &proven)?;
            write_seal(&guest, &seal, &journal, &bytes)?;
            println!("seal written to {}", seal.display());
        }
        ZkvmCommand::Parity { public_tx } => {
            println!("image id 0x{} ({} network)", hex::encode(guest.image_id()), NETWORK.name);
            parity(&guest, &public_tx)?;
        }
        // `groth16` in two processes, as risc0-zkvm's succinct_to_groth16 chains them: the Poseidon254 identity
        // receipt, then its Groth16 wrap. Run apart, each half can be checked on its own.
        #[cfg(feature = "cuda")]
        ZkvmCommand::P254 { receipt, out } => {
            let proven: Receipt = bincode::deserialize(&std::fs::read(&receipt)?)?;
            proven.verify(guest.id).context("the receipt is not a proof of this guest")?;
            let started = Instant::now();
            let p254 = risc0_zkvm::recursion::identity_p254(proven.inner.succinct()?)?;
            std::fs::write(&out, bincode::serialize(&p254)?)?;
            println!("identity_p254 receipt in {:.1?}, written to {}", started.elapsed(), out.display());
        }
        #[cfg(feature = "cuda")]
        ZkvmCommand::ShrinkWrap { p254, receipt, seal } => {
            use risc0_zkvm::sha::Digestible;
            let p254: risc0_zkvm::SuccinctReceipt<risc0_zkvm::ReceiptClaim> =
                bincode::deserialize(&std::fs::read(&p254)?)?;
            let proven: Receipt = bincode::deserialize(&std::fs::read(&receipt)?)?;
            proven.verify(guest.id).context("the receipt is not a proof of this guest")?;
            let started = Instant::now();
            let groth16 = risc0_groth16::prove::shrink_wrap(&p254.get_seal_bytes())?.to_vec();
            let groth16 = risc0_zkvm::Groth16Receipt::new(
                groth16,
                proven.inner.succinct()?.claim.clone(),
                risc0_zkvm::Groth16ReceiptVerifierParameters::default().digest(),
            );
            let wrapped = Receipt::new(risc0_zkvm::InnerReceipt::Groth16(groth16), proven.journal.bytes.clone());
            wrapped.verify(guest.id)?;
            let g = wrapped.inner.groth16()?;
            let bytes = [&g.verifier_parameters.as_bytes()[..4], &g.seal[..]].concat();
            write_seal(&guest, &seal, &wrapped.journal.bytes, &bytes)?;
            println!("Groth16 seal in {:.1?}, verified, written to {}", started.elapsed(), seal.display());
        }
    }
    Ok(())
}
