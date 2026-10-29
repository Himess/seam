//! Tests that need Zcash Testnet: run them by hand with `cargo test -p seam-chain --test network -- --ignored
//! --nocapture`. `SEAM_LWD` names the light wallet server (default `https://testnet.zec.rocks:443`) and
//! `SEAM_TEST_DATA_DIR` a directory that keeps the epoch's header file and summary cache between runs. They print
//! counts and heights only, never a peer's address.

use std::path::PathBuf;

use seam_chain::{
    ZcashNetwork,
    headers::{self, LOCATOR_DEPTH},
    history::{parse_display_hash, sha256d},
    locate::{self, DataFiles, Progress},
    lwd, p2p,
};
use seam_payment::k4::PublicTx;
use zcash_primitives::transaction::Transaction;
use zcash_protocol::consensus::BranchId;

/// Header T of crates/seam-payment's fixture: Testnet block 4,488,560.
const T: u32 = 4_488_560;
const T_HASH: &str = "00000b7d47d097937e0ce99b75fb42d85fc810bf51c7382129dce3303c5e7ba2";
const PUBLIC_TX: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../seam-payment/tests/fixtures/TEST-ONLY_testnet_ironwood_tx.json");

fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("seam-chain-network-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn lwd_url() -> String {
    std::env::var("SEAM_LWD").unwrap_or_else(|_| "https://testnet.zec.rocks:443".to_owned())
}

/// A new header file started after T, extended over more than one peer answer (160 headers each) in two calls, is
/// one chain from T on; the second call starts from the file's tail.
#[test]
#[ignore = "needs Zcash Testnet peers"]
fn headers_from_testnet_peers() {
    let path = temp_dir().join("headers.jsonl");
    let _ = std::fs::remove_file(&path);
    let mut peers = p2p::seed_peers(ZcashNetwork::Testnet);
    println!("{} seeded addresses", peers.len());
    let t_hash = parse_display_hash(T_HASH).unwrap();
    let last = headers::extend(ZcashNetwork::Testnet, &path, Some((T, t_hash)), T + 200, &mut peers).unwrap();
    assert_eq!(last, T + 200);
    let last = headers::extend(ZcashNetwork::Testnet, &path, None, T + 400, &mut peers).unwrap();
    assert_eq!(last, T + 400);
    let loaded = headers::load(&path, T + 1, T + 400, &[T + 1]).unwrap();
    assert_eq!(loaded.headers.len(), 400);
    assert_eq!(loaded.headers.get(T + 1).unwrap().prev_hash, t_hash);
    assert_eq!(sha256d(&loaded.raw[&(T + 1)]), loaded.headers.get(T + 1).unwrap().hash);
    println!("400 headers after {T}, linked; locator depth {LOCATOR_DEPTH}");
    std::fs::remove_file(&path).unwrap();
}

/// The light wallet server follows Testnet, and refuses to be taken for a Mainnet server.
#[tokio::test]
#[ignore = "needs a Testnet light wallet server"]
async fn the_light_wallet_server_follows_testnet() {
    let mut client = lwd::connect(&lwd_url()).await.unwrap();
    lwd::check_network(&mut client, ZcashNetwork::Testnet).await.unwrap();
    assert!(lwd::check_network(&mut client, ZcashNetwork::Mainnet).await.is_err());
    let tip = lwd::latest_block(&mut client).await.unwrap().height;
    println!("the server is at block {tip}");
    assert!(tip > u64::from(T));
}

/// The fixture's public transaction, located again from the network under its anchor T, gives exactly the fixture's
/// inclusion data: the NU7 epoch's 23,535 headers up to T from the P2P network and its blocks' summaries from the
/// light wallet server, rebuilt from nothing unless `SEAM_TEST_DATA_DIR` keeps them (the first run takes minutes).
#[tokio::test]
#[ignore = "needs Zcash Testnet peers and a light wallet server"]
async fn the_fixture_is_located_again_from_the_network() {
    let p: PublicTx = serde_json::from_slice(&std::fs::read(PUBLIC_TX).unwrap()).unwrap();
    let txid = Transaction::read(&p.tx[..], BranchId::Nu7).unwrap().txid();
    let network = ZcashNetwork::Testnet;
    let epoch = network.epoch(T).unwrap();
    let dir = std::env::var_os("SEAM_TEST_DATA_DIR").map_or_else(temp_dir, PathBuf::from);
    let files = DataFiles::in_dir(&dir, network, epoch);
    let mut client = lwd::connect(&lwd_url()).await.unwrap();
    lwd::check_network(&mut client, network).await.unwrap();
    let mut peers = p2p::seed_peers(network);
    let started = std::time::Instant::now();
    let mut progress = |p: Progress| match p {
        Progress::Headers { until } => println!("headers up to {until} ({:.0?})", started.elapsed()),
        Progress::HeadersDone { last } => println!("headers reach {last} ({:.0?})", started.elapsed()),
        Progress::Summaries { height, last } => println!("summaries {height} of {last} ({:.0?})", started.elapsed()),
    };
    let at = locate::locate(&mut client, network, &files, &txid, T, &mut peers, &mut progress).await.unwrap();
    println!(
        "located in {:.1?}: tx {} of {}, leaf {} of {}, path {}, peak {} of {}",
        started.elapsed(),
        at.tx_index,
        at.block_transactions,
        at.leaf_index,
        at.leaves,
        at.mmr_path.len(),
        at.peak_index,
        at.peaks.len()
    );
    assert_eq!(at.raw_tx, p.tx);
    assert_eq!(at.epoch.branch_id, p.branch_id);
    assert_eq!(at.tx_index, p.tx_index as usize);
    assert_eq!(at.txid_path, p.txid_path);
    assert_eq!(at.block_header, p.block_header);
    assert_eq!(at.leaf, p.leaf);
    assert_eq!(at.mmr_path, p.mmr_path);
    assert_eq!(at.peaks, p.peaks);
    assert_eq!(at.peak_index, p.peak_index as usize);
    assert_eq!(at.tip_header, p.tip_header);
    assert_eq!(at.tip_auth_data_root, p.tip_auth_data_root);
    assert_eq!((at.tip_height, at.leaves), (T, 23_534));

    // A second run reads both files back: no summary is rebuilt.
    let started = std::time::Instant::now();
    let mut rebuilt = 0;
    let again = locate::locate(&mut client, network, &files, &txid, T, &mut peers, &mut |p| {
        if let Progress::Summaries { .. } = p {
            rebuilt += 1;
        }
    })
    .await
    .unwrap();
    assert_eq!((again.peaks, rebuilt), (at.peaks, 0));
    println!("located again from the files in {:.1?}", started.elapsed());
}
