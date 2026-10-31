//! The history tree, the txid tree and the epochs against real chain data: the public transaction of
//! crates/seam-payment's fixture (`TEST-ONLY_testnet_ironwood_tx.json`), a real Ironwood-only v6 transaction mined in
//! Zcash Testnet block 4,488,543, with its real inclusion data under header 4,488,560.

use std::fs;

use seam_chain::{
    ZcashNetwork,
    history::{BlockSummary, Header, block_commitments, display_hash, leaf, txid_merkle_path, txid_merkle_root},
    mmr::{Mmr, PathStep, bag_peaks, fold_path, peak_ranges},
};
use seam_payment::{Inclusion, MmrStep, NU7_BRANCH_ID, check_inclusion, k4::PublicTx, sha256d};
use zcash_history::{NodeDataV3, V3, Version};
use zcash_primitives::transaction::Transaction;
use zcash_protocol::consensus::BranchId;

const PUBLIC_TX: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../seam-payment/tests/fixtures/TEST-ONLY_testnet_ironwood_tx.json");
const B: u32 = 4_488_543;
const T: u32 = 4_488_560;

fn public_tx() -> PublicTx {
    serde_json::from_slice(&fs::read(PUBLIC_TX).expect("fixture")).expect("public transaction JSON")
}

fn node(bytes: &[u8]) -> NodeDataV3 {
    V3::from_bytes(NU7_BRANCH_ID, bytes).expect("a V3 node")
}

fn source(key: &str) -> serde_json::Value {
    let json: serde_json::Value = serde_json::from_slice(&fs::read(PUBLIC_TX).expect("fixture")).expect("JSON");
    json["source"][key].clone()
}

/// B's summary: its header's hash, time and nBits, its height, and the tree roots and transaction counts its leaf
/// records.
fn summary_of_b(p: &PublicTx) -> BlockSummary {
    let header = Header::parse(&p.block_header).unwrap();
    let recorded = node(&p.leaf);
    let v2 = &recorded.v2;
    BlockSummary {
        height: B,
        hash: header.hash,
        time: header.time,
        bits: header.bits,
        sapling_root: v2.v1.end_sapling_root,
        orchard_root: v2.end_orchard_root,
        ironwood_root: recorded.end_ironwood_root,
        sapling_tx: v2.v1.sapling_tx,
        orchard_tx: v2.orchard_tx,
        ironwood_tx: recorded.ironwood_tx,
    }
}

/// B's leaf is the one `history::leaf` builds from B's header (hash, time, nBits and the work they give) and B's
/// height, with the tree roots and transaction counts it records.
#[test]
fn the_leaf_of_block_b() {
    let p = public_tx();
    assert_eq!(display_hash(&Header::parse(&p.block_header).unwrap().hash), source("block_hash_display"));
    let summary = summary_of_b(&p);
    assert_eq!(V3::to_bytes(&leaf(NU7_BRANCH_ID, &summary)), p.leaf);
    assert!(summary.ironwood_tx >= 1, "B holds the Ironwood transaction");
}

/// The inclusion rebuilt here, B's leaf from its header and the txid path from B's txids, with the real path and
/// peaks folded by this crate, is one `seam_payment::check_inclusion` accepts, for B under T.
#[test]
fn the_rebuilt_inclusion_is_the_statements() {
    let p = public_tx();
    let txid: [u8; 32] = *Transaction::read(&p.tx[..], BranchId::Nu7).unwrap().txid().as_ref();
    let txid_path = txid_merkle_path(&[p.txid_path[0], txid], 1).unwrap();
    let leaf_node = leaf(NU7_BRANCH_ID, &summary_of_b(&p));
    let path: Vec<PathStep> = p.mmr_path.iter().map(|s| (node(&s.sibling), s.sibling_is_left)).collect();
    let mut peaks: Vec<Vec<u8>> = p.peaks.clone();
    peaks[p.peak_index as usize] = V3::to_bytes(&fold_path(&leaf_node, &path));
    let mmr_path: Vec<MmrStep> = path
        .iter()
        .map(|(n, left)| MmrStep {
            sibling: V3::to_bytes(n),
            sibling_is_left: *left,
        })
        .collect();
    let included = check_inclusion(
        &txid,
        &Inclusion {
            branch_id: NU7_BRANCH_ID,
            tx_index: 1,
            txid_path: &txid_path,
            block_header: &p.block_header,
            leaf: &V3::to_bytes(&leaf_node),
            mmr_path: &mmr_path,
            peaks: &peaks,
            peak_index: p.peak_index,
            tip_header: &p.tip_header,
            tip_auth_data_root: &p.tip_auth_data_root,
        },
    )
    .unwrap();
    assert_eq!((included.block_height, included.tip_hash), (u64::from(B), sha256d(&p.tip_header)));
}

/// B holds two transactions; the payment is the second. The txid tree over them gives the fixture's path and B's
/// `hashMerkleRoot`.
#[test]
fn the_txid_path_in_block_b() {
    let p = public_tx();
    let tx = Transaction::read(&p.tx[..], BranchId::Nu7).expect("a v6 transaction");
    let txid: [u8; 32] = *tx.txid().as_ref();
    assert_eq!((p.tx_index, p.txid_path.len()), (1, 1));
    let txids = [p.txid_path[0], txid];
    assert_eq!(txid_merkle_root(&txids).unwrap(), Header::parse(&p.block_header).unwrap().merkle_root);
    assert_eq!(txid_merkle_path(&txids, 1).unwrap(), p.txid_path);
}

/// The real path leads from B's leaf to its peak, and the left fold of the real peaks, with T's auth data root, gives
/// T's `hashBlockCommitments`.
#[test]
fn the_history_t_commits_to() {
    let p = public_tx();
    let path: Vec<PathStep> = p.mmr_path.iter().map(|s| (node(&s.sibling), s.sibling_is_left)).collect();
    let peaks: Vec<NodeDataV3> = p.peaks.iter().map(|b| node(b)).collect();
    assert_eq!(V3::to_bytes(&fold_path(&node(&p.leaf), &path)), p.peaks[p.peak_index as usize]);
    let root = bag_peaks(&peaks).unwrap();
    let tip = Header::parse(&p.tip_header).unwrap();
    assert_eq!(block_commitments(&V3::hash(&root), &p.tip_auth_data_root), tip.block_commitments);
    assert_eq!(display_hash(&tip.hash), source("tip_hash_display"));
    // The root covers the epoch's blocks up to T - 1.
    assert_eq!(root.v2.v1.start_height, u64::from(ZcashNetwork::Testnet.epoch(T).unwrap().activation));
    assert_eq!(root.v2.v1.end_height, u64::from(T - 1));
}

/// The fixture's shape is the one the epoch gives: T commits to the 23,534 blocks from NU7's activation (4,465,026)
/// to T - 1, in 11 peaks; B is leaf 23,517, under the eighth peak (32 leaves), so its path has 5 steps, on the sides
/// the bits of its place under that peak give. `Mmr::path` finds the same.
#[test]
fn the_place_of_block_b_in_the_epoch() {
    let p = public_tx();
    let epoch = ZcashNetwork::Testnet.epoch(T).unwrap();
    assert_eq!(epoch.branch_id, p.branch_id);
    let leaves = (T - epoch.activation) as usize;
    let index = (B - epoch.activation) as usize;
    assert_eq!((leaves, index), (23_534, 23_517));
    let ranges = peak_ranges(leaves);
    assert_eq!(ranges.len(), p.peaks.len());
    assert_eq!(ranges[p.peak_index as usize], (23_488, 32));

    // Synthetic leaves around B's real one: the shape alone is compared.
    let mut ls: Vec<NodeDataV3> = (0..leaves as u32)
        .map(|i| {
            leaf(
                NU7_BRANCH_ID,
                &BlockSummary {
                    height: epoch.activation + i,
                    hash: [(i % 251) as u8; 32],
                    time: 1_791_138_105 + 25 * i,
                    bits: 0x1f07_ffff,
                    sapling_root: [0; 32],
                    orchard_root: [0; 32],
                    ironwood_root: [0; 32],
                    sapling_tx: 0,
                    orchard_tx: 0,
                    ironwood_tx: 0,
                },
            )
        })
        .collect();
    ls[index] = node(&p.leaf);
    let mmr = Mmr::new(ls).unwrap();
    let (path, peak) = mmr.path(index).unwrap();
    assert_eq!(peak, p.peak_index as usize);
    let sides: Vec<bool> = path.iter().map(|(_, left)| *left).collect();
    let real: Vec<bool> = p.mmr_path.iter().map(|s| s.sibling_is_left).collect();
    assert_eq!(sides, real);
    assert_eq!(mmr.peaks().len(), p.peaks.len());
}
