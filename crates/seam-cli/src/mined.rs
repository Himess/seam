//! `seam mined`: whether a payment is mined, and at which height, for the transaction an opening or a txid file names,
//! as one line of JSON for tools: `{"mined":<height>,"tip":<height>}`, or `"mined":null` while it is not. The txid is
//! read from a file and never printed: it stays off command lines and out of logs.

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use clap::Args;
use seam_chain::{lwd, witness::Opening};
use zcash_protocol::TxId;

use crate::read_secret_text;

#[derive(Args)]
pub struct MinedArgs {
    /// An opening written by `seam pay`, which names its transaction.
    #[arg(long, required_unless_present = "txid_file", conflicts_with = "txid_file")]
    opening: Option<PathBuf>,
    /// A file with the transaction's id, 64 hex digits as block explorers show it.
    #[arg(long)]
    txid_file: Option<PathBuf>,
    /// The Zcash network: this build's (testnet, or mainnet with the `mainnet` feature).
    #[arg(long, value_parser = ["testnet", "mainnet"])]
    network: Option<String>,
    /// The light wallet server (default: https://testnet.zec.rocks:443 on Testnet, https://zec.rocks:443 on Mainnet).
    #[arg(long)]
    lwd: Option<String>,
}

pub async fn run(args: MinedArgs) -> Result<()> {
    let network = crate::network(args.network.as_deref())?;
    let text = match (&args.opening, &args.txid_file) {
        (Some(path), None) => {
            // serde_json's message can quote the file's text: only its position is reported.
            let opening: Opening = serde_json::from_str(&read_secret_text(path)?).map_err(|e| {
                anyhow!("the opening file is not an opening (line {}, column {})", e.line(), e.column())
            })?;
            opening.txid.ok_or_else(|| anyhow!("the opening does not name its transaction"))?
        }
        (None, Some(path)) => read_secret_text(path)?,
        _ => bail!("give one of --opening and --txid-file"),
    };
    let txid = TxId::from_hex(text.trim()).context("a txid: 64 hex digits, as block explorers show it")?;
    let url = args.lwd.as_deref().unwrap_or(crate::default_lwd(network));
    crate::warn_if_plaintext(url);
    let mut client = lwd::connect(url).await?;
    lwd::check_network(&mut client, network).await?;
    let tip = lwd::latest_block(&mut client).await?.height;
    // A transaction the server doesn't know is an error, like a transport failure: no answer either way.
    let (_, height) = lwd::raw_transaction(&mut client, &txid).await?;
    let mined = if height == 0 {
        "null".to_owned()
    } else {
        height.to_string()
    };
    println!("{{\"mined\":{mined},\"tip\":{tip}}}");
    Ok(())
}
