//! `seam witness`: a payment witness built on this machine from public chain data and either the payer's opening or
//! the receiver's viewing key (trial decryption here), checked with the statement before it is written.

use std::path::PathBuf;

use anyhow::{Context, Result, bail, ensure};
use clap::Args;
use seam_chain::{
    history::{display_hash, sha256d},
    locate::{self, DataFiles, Progress},
    lwd, p2p,
    witness::{Opening, find_payments, from_opening, ufvk_fvk},
};
use zcash_protocol::TxId;

use crate::{memo::parse_ref, read_secret_text, write_private};

#[derive(Args)]
pub struct WitnessArgs {
    /// The payment transaction's id, as block explorers show it.
    #[arg(long)]
    txid: String,
    /// A JSON file with the payer's opening of the payment note: {action_index, receiver (hex), value, rseed (hex)}.
    #[arg(long, required_unless_present = "ufvk_file", conflicts_with = "ufvk_file")]
    opening: Option<PathBuf>,
    /// A file with the receiver's unified full viewing key: the note is found by trial decryption on this machine.
    #[arg(long)]
    ufvk_file: Option<PathBuf>,
    /// A file with the receiver commitment's 32-byte salt, in hex.
    #[arg(long)]
    salt_file: PathBuf,
    /// The height of header T the proof is anchored to: after the payment's block and in its epoch, a header the
    /// relay holds with enough work on top (docs/PAYMENTS.md § 7).
    #[arg(long)]
    anchor: u32,
    /// Where the witness is written (JSON). It holds the receiver and the salt: keep it on machines you control.
    #[arg(long)]
    out: PathBuf,
    /// The paymentRef the payment's memo must name (hex); it also picks the payment when a viewing key finds several.
    #[arg(long)]
    payment_ref: Option<String>,
    /// The receiver commitment the payment must pay (hex), such as the escrow order's.
    #[arg(long)]
    receiver_commitment: Option<String>,
    /// The Zcash network: this build's (testnet, or mainnet with the `mainnet` feature).
    #[arg(long, value_parser = ["testnet", "mainnet"])]
    network: Option<String>,
    /// The light wallet server (default: https://testnet.zec.rocks:443 on Testnet, https://zec.rocks:443 on Mainnet).
    #[arg(long)]
    lwd: Option<String>,
    /// Where the epoch's header file and block summaries are kept between runs (public chain data only).
    #[arg(long, default_value = "seam-data")]
    data_dir: PathBuf,
}

/// Where the note's opening comes from.
enum Source {
    Opening(Opening),
    ViewingKey(orchard::keys::FullViewingKey),
}

pub async fn run(args: WitnessArgs) -> Result<()> {
    let network = crate::network(args.network.as_deref())?;
    let salt = parse_ref(&read_secret_text(&args.salt_file)?).context("the salt file: 64 hex digits")?;
    let source = match (&args.opening, &args.ufvk_file) {
        (Some(path), None) => Source::Opening(
            serde_json::from_str(&read_secret_text(path)?)
                .context("the opening file: {action_index, receiver, value, rseed}")?,
        ),
        (None, Some(path)) => Source::ViewingKey(ufvk_fvk(network, &read_secret_text(path)?)?),
        _ => bail!("give one of --opening and --ufvk-file"),
    };
    let txid = TxId::from_hex(args.txid.trim()).context("--txid: 64 hex digits, as block explorers show it")?;
    let payment_ref = args.payment_ref.as_deref().map(parse_ref).transpose().context("--payment-ref")?;
    let commitment = args.receiver_commitment.as_deref().map(parse_ref).transpose().context("--receiver-commitment")?;
    let epoch = network.epoch(args.anchor)?;
    std::fs::create_dir_all(&args.data_dir)
        .with_context(|| format!("create the data directory {}", args.data_dir.display()))?;
    let files = DataFiles::in_dir(&args.data_dir, network, epoch);

    let url = args.lwd.as_deref().unwrap_or(crate::default_lwd(network));
    let mut client = lwd::connect(url).await?;
    lwd::check_network(&mut client, network).await?;
    let mut peers = p2p::seed_peers(network);
    let at = locate::locate(&mut client, network, &files, &txid, args.anchor, &mut peers, &mut |p| match p {
        Progress::Headers { until } => eprintln!("headers: from Zcash {network} peers, up to block {until}"),
        Progress::HeadersDone { last } => eprintln!("headers: up to block {last}"),
        Progress::Summaries { height, last } => eprintln!("block summaries: up to block {height} of {last}"),
    })
    .await?;

    let opening = match source {
        Source::Opening(o) => o,
        Source::ViewingKey(fvk) => {
            let found: Vec<_> = find_payments(&at.transaction()?, &fvk)?
                .into_iter()
                .filter(|f| payment_ref.is_none_or(|r| f.payment_ref == r))
                .collect();
            match found.as_slice() {
                [] => bail!("the viewing key finds no payment with a Seam memo in the transaction"),
                [one] => one.opening.clone(),
                more => bail!(
                    "the viewing key finds {} payments in the transaction: name one with --payment-ref",
                    more.len()
                ),
            }
        }
    };
    let (witness, journal) = from_opening(&at, &opening, salt)?;
    if let Some(r) = payment_ref {
        ensure!(journal.payment_ref == r, "the payment's memo names another paymentRef");
    }
    if let Some(c) = commitment {
        ensure!(journal.receiver_commitment == c, "the payment pays another receiver commitment");
    }
    write_private(&args.out, &serde_json::to_vec(&witness)?)?;

    // Public data only: the anchor, the history's size and the journal. Not the payment's block: only T is public.
    println!("anchor              block {}, {}", at.tip_height, display_hash(&sha256d(&at.tip_header)));
    println!(
        "history             {} blocks of epoch 0x{:08x} from block {}",
        at.leaves, epoch.branch_id, epoch.activation
    );
    println!("journal             0x{}", hex::encode(journal.abi_encode()));
    println!("paymentRef          0x{}", hex::encode(journal.payment_ref));
    println!("receiverCommitment  0x{}", hex::encode(journal.receiver_commitment));
    println!("zatoshi             {}", journal.zatoshi);
    println!("witness written to {}: it holds the receiver and the salt, keep it private", args.out.display());
    Ok(())
}
