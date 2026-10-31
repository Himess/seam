//! `seam pay`: a Seam payment from a disposable Testnet wallet, and the wallet itself (`--new-wallet`). The payment is
//! one Ironwood note to the receiver with the memo of the `paymentRef` and `ovk = None`, Ironwood-only, checked with
//! the statement before it is sent; its opening, with its txid, is written before it is sent, since nobody can
//! recover the note from the chain afterwards. Testnet only: the wallet's key sits in a plain file.
//!
//! ```text
//! seam pay --wallet <wallet.json> --new-wallet --address-out <address.txt> [--ufvk-out <ufvk.txt>]
//! seam pay --wallet <wallet.json> --to <ua> --zat <n> --ref <paymentRef> --opening <opening.json>
//! ```

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, ensure};
use clap::Args;
use orchard::keys::{FullViewingKey, Scope, SpendingKey};
use rand_core::{OsRng, RngCore};
use seam_chain::{
    ZcashNetwork, lwd,
    pay::{authorize, build_payment, check_payment, proving_key, select_inputs, serialize},
    scan::ScanState,
    witness::fvk_ufvk,
};
use serde::{Deserialize, Serialize};
use zcash_keys::address::UnifiedAddress;
use zcash_protocol::consensus::BlockHeight;

use crate::{
    memo::{ironwood_receiver, parse_ref},
    read_secret_text, write_private,
};

#[derive(Args)]
pub struct PayArgs {
    /// The disposable wallet: a JSON file {spending_key, birthday}, which --new-wallet creates. Keep it outside any
    /// repository.
    #[arg(long)]
    wallet: PathBuf,
    /// Creates the wallet (a random spending key, born at the server's tip) and writes its unified address, to fund it
    /// with, to --address-out.
    #[arg(long, requires = "address_out", conflicts_with_all = ["to", "zat", "payment_ref", "opening"])]
    new_wallet: bool,
    /// Where --new-wallet writes the wallet's unified address.
    #[arg(long, requires = "new_wallet")]
    address_out: Option<PathBuf>,
    /// Where --new-wallet writes the wallet's unified full viewing key, for `seam witness --ufvk-file` when the wallet
    /// receives a payment. Keep it private: it shows every payment to the wallet.
    #[arg(long, requires = "new_wallet")]
    ufvk_out: Option<PathBuf>,
    /// The receiver's unified address: its Orchard-protocol receiver is paid, in the Ironwood pool.
    #[arg(long, required_unless_present = "new_wallet")]
    to: Option<String>,
    /// The payment's value, in zatoshi.
    #[arg(long, required_unless_present = "new_wallet")]
    zat: Option<u64>,
    /// The paymentRef the memo names (64 hex digits).
    #[arg(long = "ref", required_unless_present = "new_wallet")]
    payment_ref: Option<String>,
    /// Where the payment's opening and txid are written, before it is sent: `seam witness --opening` reads it. Never
    /// overwritten.
    #[arg(long, required_unless_present = "new_wallet")]
    opening: Option<PathBuf>,
    /// The Zcash network: testnet only.
    #[arg(long, value_parser = ["testnet", "mainnet"])]
    network: Option<String>,
    /// The light wallet server (default: https://testnet.zec.rocks:443).
    #[arg(long)]
    lwd: Option<String>,
}

/// A disposable wallet's file, as Zecured's wallet wrote it.
#[derive(Serialize, Deserialize)]
struct WalletFile {
    /// The spending key, hex.
    spending_key: String,
    /// The first block that may hold its notes.
    birthday: u32,
}

struct Wallet {
    sk: SpendingKey,
    fvk: FullViewingKey,
    birthday: u32,
}

fn load_wallet(path: &std::path::Path) -> Result<Wallet> {
    // serde_json's message can quote the file's text, here a key: only its position is reported.
    let f: WalletFile = serde_json::from_str(&read_secret_text(path)?).map_err(|e| {
        anyhow!("the wallet file is not {{spending_key, birthday}} (line {}, column {})", e.line(), e.column())
    })?;
    let bytes: [u8; 32] = hex::decode(f.spending_key.trim())
        .ok()
        .and_then(|b| b.try_into().ok())
        .context("the wallet's key")?;
    let sk = Option::<SpendingKey>::from(SpendingKey::from_bytes(bytes)).context("the wallet's key")?;
    Ok(Wallet {
        fvk: FullViewingKey::from(&sk),
        sk,
        birthday: f.birthday,
    })
}

pub fn run(args: PayArgs) -> Result<()> {
    let network = crate::network(args.network.as_deref())?;
    ensure!(network == ZcashNetwork::Testnet, "seam pay pays on Testnet only, from a disposable wallet");
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().context("start a runtime")?;
    let url = args.lwd.as_deref().unwrap_or(crate::default_lwd(network));
    crate::warn_if_plaintext(url);
    let connect = || async {
        let mut client = lwd::connect(url).await?;
        lwd::check_network(&mut client, network).await?;
        let tip = u32::try_from(lwd::latest_block(&mut client).await?.height)?;
        Ok::<_, anyhow::Error>((client, tip))
    };

    if args.new_wallet {
        let address_out = args.address_out.context("--address-out")?;
        ensure!(!args.wallet.exists(), "{} exists: a new wallet never replaces one", args.wallet.display());
        let (_, tip) = runtime.block_on(connect())?;
        let sk = loop {
            let mut bytes = [0u8; 32];
            OsRng.fill_bytes(&mut bytes);
            if let Some(sk) = Option::<SpendingKey>::from(SpendingKey::from_bytes(bytes)) {
                break sk;
            }
        };
        let file = WalletFile {
            spending_key: hex::encode(sk.to_bytes()),
            birthday: tip,
        };
        write_private(&args.wallet, &serde_json::to_vec_pretty(&file)?)?;
        let receiver = FullViewingKey::from(&sk).address_at(0u32, Scope::External);
        let ua = UnifiedAddress::from_receivers(Some(receiver), None, None, None, None)
            .context("a unified address")?
            .encode(&network.params());
        write_private(&address_out, format!("{ua}\n").as_bytes())?;
        println!("wallet created, born at block {tip}; its unified address is in {}", address_out.display());
        if let Some(ufvk_out) = args.ufvk_out {
            write_private(&ufvk_out, format!("{}\n", fvk_ufvk(network, &FullViewingKey::from(&sk))?).as_bytes())?;
            println!("its unified full viewing key is in {}: keep it private", ufvk_out.display());
        }
        return Ok(());
    }

    let (Some(to), Some(zat), Some(payment_ref), Some(opening_out)) =
        (args.to, args.zat, args.payment_ref, args.opening)
    else {
        return Err(anyhow!("--to, --zat, --ref and --opening name the payment"));
    };
    let payment_ref = parse_ref(&payment_ref).context("--ref")?;
    ensure!(!opening_out.exists(), "{} exists: an opening is never overwritten", opening_out.display());
    let (recipient, other_receivers) = ironwood_receiver(network, &to).context("--to")?;
    if other_receivers {
        eprintln!("note: the address also has other receivers; the payment goes to its Orchard-protocol receiver");
    }
    let wallet = load_wallet(&args.wallet)?;

    // The wallet's notes, from its birthday to the server's tip; the tree must be the server's.
    let (mut client, state) = runtime.block_on(async {
        let (mut client, tip) = connect().await?;
        let mut state = ScanState::from_birthday(&mut client, wallet.fvk.clone(), wallet.birthday).await?;
        state.scan_to(&mut client, tip).await?;
        state.check_against_server(&mut client).await?;
        Ok::<_, anyhow::Error>((client, state))
    })?;
    let unspent = state.unspent();
    eprintln!(
        "wallet: {} unspent notes, {} zatoshi, scanned to block {}",
        unspent.len(),
        state.balance(),
        state.scanned_to
    );
    let inputs = select_inputs(&unspent, zat)?;
    let target = BlockHeight::from_u32(state.scanned_to + 1);
    let unsigned =
        build_payment(&network.params(), target, &wallet.fvk, state.anchor(), &inputs, recipient, zat, &payment_ref)?;
    let (fee, change) = (unsigned.fee, unsigned.change);
    let mut opening = unsigned.opening.clone();
    eprintln!("proving the payment (Halo 2)");
    let tx = authorize(unsigned, &wallet.sk, &proving_key())?;
    // A payment the statement could not prove is never sent.
    check_payment(network.statement(), &tx, &opening, &payment_ref)?;
    opening.txid = Some(tx.txid().to_string());
    write_private(&opening_out, &serde_json::to_vec_pretty(&opening)?)?;
    runtime.block_on(lwd::send_transaction(&mut client, serialize(&tx)?))?;
    println!(
        "paid {zat} zatoshi for paymentRef 0x{} (fee {fee}, change {change}); the light wallet server accepted it",
        hex::encode(payment_ref)
    );
    println!("opening and txid written to {}: keep it private", opening_out.display());
    Ok(())
}
