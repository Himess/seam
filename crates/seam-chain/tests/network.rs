//! Tests that need Zcash Testnet: run them by hand with `cargo test -p seam-chain --test network -- --ignored`. They
//! print counts and heights only, never a peer's address.

use seam_chain::{
    ZcashNetwork,
    headers::{self, LOCATOR_DEPTH},
    history::{parse_display_hash, sha256d},
    p2p,
};

/// Header T of crates/seam-payment's fixture: Testnet block 4,488,560.
const T: u32 = 4_488_560;
const T_HASH: &str = "00000b7d47d097937e0ce99b75fb42d85fc810bf51c7382129dce3303c5e7ba2";

fn temp_path(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("seam-chain-network-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// A new header file started after T, extended over more than one peer answer (160 headers each) in two calls, is
/// one chain from T on; the second call starts from the file's tail.
#[test]
#[ignore = "needs Zcash Testnet peers"]
fn headers_from_testnet_peers() {
    let path = temp_path("headers.jsonl");
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
