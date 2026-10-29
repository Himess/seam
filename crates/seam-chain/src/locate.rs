//! Where a mined transaction is: its block B, and B's place in the history of its epoch that a later header T
//! commits to. This is the inclusion part of a payment witness. Ported from Zecured's `witness::locate`
//! (`signer/crates/deposit-witness/src/witness.rs` at 73de9f909e270e8aea10c2b221905f62a6a4c4df).
//!
//! [`assemble`] builds it from chain data and checks every part of it against the headers; [`locate`] fetches that
//! data: headers from the P2P network, compact blocks, tree states and transactions from a light wallet server.

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use seam_payment::MmrStep;
use zcash_history::{V3, Version};
use zcash_primitives::transaction::Transaction;
use zcash_protocol::{TxId, consensus::BranchId};

use crate::{
    chain, headers,
    history::{self, BlockSummary, Header, block_commitments},
    lwd::{self, LwdClient},
    mmr::Mmr,
    network::{Epoch, ZcashNetwork},
};

/// A transaction and where it is, under header T.
#[derive(Clone, Debug)]
pub struct Located {
    /// The Zcash network the transaction was read from, and its epoch.
    pub network: ZcashNetwork,
    pub epoch: Epoch,
    pub raw_tx: Vec<u8>,
    /// Block B's height, the transaction's position in it, and how many transactions it holds.
    pub height: u32,
    pub tx_index: usize,
    pub block_transactions: usize,
    pub txid_path: Vec<[u8; 32]>,
    pub block_header: Vec<u8>,
    /// B's history-tree leaf (ZIP 221 V3, serialized), its position among the history's leaves, its path to its peak,
    /// all the peaks (oldest first) and which one the path reaches.
    pub leaf: Vec<u8>,
    pub leaf_index: usize,
    pub leaves: usize,
    pub mmr_path: Vec<MmrStep>,
    pub peaks: Vec<Vec<u8>>,
    pub peak_index: usize,
    /// Header T, its height and its block's auth data root (ZIP 244).
    pub tip_height: u32,
    pub tip_header: Vec<u8>,
    pub tip_auth_data_root: [u8; 32],
}

impl Located {
    /// The transaction, parsed (v5 and v6 transactions carry their own branch id; the epoch's is passed for older
    /// ones).
    pub fn transaction(&self) -> Result<Transaction> {
        let branch = BranchId::try_from(self.epoch.branch_id).map_err(|e| anyhow!("{e}"))?;
        Transaction::read(&self.raw_tx[..], branch).context("the transaction does not parse")
    }
}

/// The public chain data a transaction is located with.
#[derive(Clone, Debug)]
pub struct ChainData {
    /// The epoch of B and T, and the summaries of its blocks up to T - 1, oldest first.
    pub epoch: Epoch,
    pub summaries: Vec<BlockSummary>,
    /// The transaction, and the height of the block B that mined it.
    pub raw_tx: Vec<u8>,
    pub height: u32,
    /// B's header and its transactions' txids (internal byte order), in block order.
    pub block_header: Vec<u8>,
    pub block_txids: Vec<[u8; 32]>,
    /// T's header, its transactions' txids and their authorizing data commitments (ZIP 244), in block order.
    pub tip_header: Vec<u8>,
    pub tip_txids: Vec<[u8; 32]>,
    pub tip_auth_commitments: Vec<[u8; 32]>,
}

/// The transaction of `d` located under T, every part checked: B's header is the summarized block at its height and
/// its `hashMerkleRoot` is its transactions' txid tree, which holds the transaction; T follows the last summarized
/// block, its `hashMerkleRoot` is its transactions' txid tree, and its `hashBlockCommitments` commits to the history
/// rebuilt from the summaries and to its auth data root.
pub fn assemble(network: ZcashNetwork, d: &ChainData) -> Result<Located> {
    let epoch = d.epoch;
    ensure!(network.epochs().contains(&epoch), "an epoch Zcash {network} does not have");
    let (Some(first), Some(last)) = (d.summaries.first(), d.summaries.last()) else {
        bail!("an empty history");
    };
    ensure!(first.height == epoch.activation, "the summaries do not start at the epoch's activation");
    ensure!(
        d.summaries.iter().enumerate().all(|(i, s)| s.height == epoch.activation + i as u32),
        "the summaries are not of consecutive blocks"
    );
    let tip_height = last.height + 1;
    ensure!(epoch.contains(tip_height), "header T is not in the epoch of the summaries");
    // Never the heights themselves: an error a payer logs must not say which block holds its payment.
    ensure!(
        d.height >= epoch.activation && d.height < tip_height,
        "the transaction is not below the anchor in the anchor's epoch"
    );

    let branch = BranchId::try_from(epoch.branch_id).map_err(|e| anyhow!("{e}"))?;
    let tx = Transaction::read(&d.raw_tx[..], branch).context("the transaction does not parse")?;
    let txid: [u8; 32] = *tx.txid().as_ref();

    let leaf_index = (d.height - epoch.activation) as usize;
    let block = Header::parse(&d.block_header).context("block B's header")?;
    ensure!(block.hash == d.summaries[leaf_index].hash, "block B's header is not the summarized block");
    ensure!(
        history::txid_merkle_root(&d.block_txids)? == block.merkle_root,
        "block B's transactions are not the ones its header commits to"
    );
    let tx_index = d.block_txids.iter().position(|t| *t == txid).context("the transaction is not in its block")?;
    let txid_path = history::txid_merkle_path(&d.block_txids, tx_index)?;

    let tip = Header::parse(&d.tip_header).context("header T")?;
    ensure!(tip.prev_hash == last.hash, "header T does not follow the last summarized block");
    ensure!(
        history::txid_merkle_root(&d.tip_txids)? == tip.merkle_root,
        "T's transactions are not the ones its header commits to"
    );
    ensure!(
        d.tip_auth_commitments.len() == d.tip_txids.len(),
        "an auth data commitment for each of T's transactions"
    );
    let tip_auth_data_root = history::auth_data_root(&d.tip_auth_commitments)?;

    let leaves: Vec<_> = d.summaries.iter().map(|s| history::leaf(epoch.branch_id, s)).collect();
    let mmr = Mmr::new(leaves)?;
    ensure!(
        block_commitments(&V3::hash(&mmr.root()), &tip_auth_data_root) == tip.block_commitments,
        "the history rebuilt from the chain data is not the one header T commits to"
    );
    let (path, peak_index) = mmr.path(leaf_index)?;
    let peaks = mmr.peaks();
    Ok(Located {
        network,
        epoch,
        raw_tx: d.raw_tx.clone(),
        height: d.height,
        tx_index,
        block_transactions: d.block_txids.len(),
        txid_path,
        block_header: d.block_header.clone(),
        leaf: V3::to_bytes(mmr.leaf(leaf_index).expect("a leaf of the history")),
        leaf_index,
        leaves: mmr.len(),
        mmr_path: path
            .iter()
            .map(|(n, left)| MmrStep {
                sibling: V3::to_bytes(n),
                sibling_is_left: *left,
            })
            .collect(),
        peaks: peaks.iter().map(V3::to_bytes).collect(),
        peak_index,
        tip_height,
        tip_header: d.tip_header.clone(),
        tip_auth_data_root,
    })
}

/// Where the chain data of one epoch is kept between runs: its header file and its summary cache. Both hold public
/// data only.
#[derive(Clone, Debug)]
pub struct DataFiles {
    pub headers: PathBuf,
    pub summaries: PathBuf,
}

impl DataFiles {
    /// The files of `network`'s `epoch` in directory `dir`.
    pub fn in_dir(dir: &Path, network: ZcashNetwork, epoch: Epoch) -> Self {
        let stem = format!("{network}-{:08x}-{}", epoch.branch_id, epoch.activation);
        Self {
            headers: dir.join(format!("{stem}.headers.jsonl")),
            summaries: dir.join(format!("{stem}.summaries.jsonl")),
        }
    }
}

/// What [`locate`] is doing, for a progress display: heights and counts only.
#[derive(Clone, Copy, Debug)]
pub enum Progress {
    /// The header file is being extended to this height.
    Headers { until: u32 },
    /// The header file reaches this height.
    HeadersDone { last: u32 },
    /// Block summaries are built up to `height`, of `last`.
    Summaries { height: u32, last: u32 },
}

fn txid_bytes(bytes: &[u8]) -> Result<[u8; 32]> {
    bytes.try_into().map_err(|_| anyhow!("a compact transaction's txid is not 32 bytes"))
}

/// Locates transaction `txid` of Zcash `network` under header `tip`, after its block and in its epoch (the relay must
/// hold that header with enough work on top of it). The epoch's header file and summary cache in `files` are extended
/// as needed: headers from `peers` (reordered, the last that answered first), the rest from the light wallet server.
pub async fn locate(
    client: &mut LwdClient,
    network: ZcashNetwork,
    files: &DataFiles,
    txid: &TxId,
    tip: u32,
    peers: &mut Vec<SocketAddr>,
    progress: &mut dyn FnMut(Progress),
) -> Result<Located> {
    let epoch = network.epoch(tip)?;
    ensure!(tip > epoch.activation, "no block of the epoch is below the anchor");
    let (raw_tx, height) = lwd::raw_transaction(client, txid).await?;
    ensure!(height != 0, "the transaction is not mined yet");
    ensure!(height != u64::MAX, "the transaction is mined on a branch that is not the main chain");
    let height = u32::try_from(height)?;
    ensure!(
        height >= epoch.activation && height < tip,
        "the transaction is not below the anchor in the anchor's epoch"
    );
    let server_tip = u32::try_from(lwd::latest_block(client).await?.height)?;
    ensure!(tip <= server_tip, "the anchor is not mined yet: the light wallet server is at block {server_tip}");

    // The headers from the epoch's activation to T, from the P2P network.
    if headers::last_height(&files.headers)?.is_none_or(|last| last < tip) {
        progress(Progress::Headers { until: tip });
        let start = match headers::last_height(&files.headers)? {
            Some(_) => None,
            None => {
                let before = epoch.activation - 1;
                let block = lwd::compact_blocks(client, before, before).await?.remove(0);
                Some((before, txid_bytes(&block.hash).context("the server sent no hash for a block")?))
            }
        };
        let path = files.headers.clone();
        let mut moved = std::mem::take(peers);
        let (last, back) = tokio::task::spawn_blocking(move || {
            let last = headers::extend(network, &path, start, tip, &mut moved);
            (last, moved)
        })
        .await?;
        *peers = back;
        let last = last?;
        progress(Progress::HeadersDone { last });
        ensure!(last >= tip, "the Zcash peers' best chain ends at block {last}, below the anchor");
    }
    let loaded = headers::load(&files.headers, epoch.activation, tip, &[height, tip])?;

    // The summaries of the epoch's blocks up to T - 1.
    let summaries = chain::summaries(client, &loaded.headers, &files.summaries, epoch, tip - 1, &mut |h| {
        progress(Progress::Summaries {
            height: h,
            last: tip - 1,
        })
    })
    .await?;

    // B's transactions; T's transactions and their authorizing data. B and T must be the header file's blocks: a
    // file whose tail was reorganised away after it was written is not followed again below its last header.
    let header_hash = |h: u32| loaded.headers.get(h).map(|x| x.hash).context("a header of the file");
    let stale =
        || format!("the header file is not the light wallet server's chain: delete {}", files.headers.display());
    let block = lwd::compact_blocks(client, height, height).await?.remove(0);
    ensure!(block.hash == header_hash(height)?, "{}", stale());
    let block_txids = block.vtx.iter().map(|t| txid_bytes(&t.txid)).collect::<Result<Vec<_>>>()?;
    let tip_block = lwd::compact_blocks(client, tip, tip).await?.remove(0);
    ensure!(tip_block.hash == header_hash(tip)?, "{}", stale());
    let tip_txids = tip_block.vtx.iter().map(|t| txid_bytes(&t.txid)).collect::<Result<Vec<_>>>()?;
    let branch = BranchId::try_from(epoch.branch_id).map_err(|e| anyhow!("{e}"))?;
    let mut tip_auth_commitments = Vec::with_capacity(tip_txids.len());
    for t in &tip_txids {
        let (raw, _) = lwd::raw_transaction(client, &TxId::from_bytes(*t)).await?;
        let tx = Transaction::read(&raw[..], branch).context("a transaction of block T does not parse")?;
        ensure!(tx.txid().as_ref() == t, "the server sent another transaction than one of block T's");
        tip_auth_commitments.push(tx.auth_commitment().as_bytes().try_into()?);
    }

    let mut raw = loaded.raw;
    assemble(
        network,
        &ChainData {
            epoch,
            summaries,
            raw_tx,
            height,
            block_header: raw.remove(&height).context("B's header")?,
            block_txids,
            tip_header: raw.remove(&tip).context("T's header")?,
            tip_txids,
            tip_auth_commitments,
        },
    )
}

#[cfg(test)]
mod tests {
    use seam_payment::{Inclusion, check_inclusion, k4::PublicTx};

    use super::*;
    use crate::{headers::tests::synthetic_header, history::sha256d};

    /// Synthetic chain data around the fixture's real transaction: `n` summarized blocks from Testnet's NU7
    /// activation, B at `b` holding the transaction second of three, and T after them.
    fn chain_data(n: u32, b: u32) -> ChainData {
        let p: PublicTx = serde_json::from_slice(
            &std::fs::read(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../seam-payment/tests/fixtures/TEST-ONLY_testnet_ironwood_tx.json"
            ))
            .unwrap(),
        )
        .unwrap();
        let txid = *Transaction::read(&p.tx[..], BranchId::Nu7).unwrap().txid().as_ref();
        let epoch = ZcashNetwork::Testnet.epoch(4_488_560).unwrap();
        let block_txids = vec![[1; 32], txid, [3; 32]];
        let (mut prev, mut summaries, mut block_header) = ([0xee; 32], vec![], vec![]);
        for i in 0..n {
            let mut raw = synthetic_header(prev, i as u8);
            if i == b {
                raw[36..68].copy_from_slice(&history::txid_merkle_root(&block_txids).unwrap());
                block_header = raw.clone();
            }
            let header = Header::parse(&raw).unwrap();
            summaries.push(BlockSummary {
                height: epoch.activation + i,
                hash: header.hash,
                time: header.time + i,
                bits: header.bits,
                sapling_root: [i as u8; 32],
                orchard_root: [0; 32],
                ironwood_root: [7; 32],
                sapling_tx: 0,
                orchard_tx: 0,
                ironwood_tx: u64::from(i == b),
            });
            prev = header.hash;
        }
        let tip_txids = vec![[9; 32], [8; 32]];
        let tip_auth_commitments = vec![[5; 32], [6; 32]];
        let leaves: Vec<_> = summaries.iter().map(|s| history::leaf(epoch.branch_id, s)).collect();
        let root = V3::hash(&Mmr::new(leaves).unwrap().root());
        let mut tip_header = synthetic_header(prev, 0xff);
        tip_header[36..68].copy_from_slice(&history::txid_merkle_root(&tip_txids).unwrap());
        let auth = history::auth_data_root(&tip_auth_commitments).unwrap();
        tip_header[68..100].copy_from_slice(&block_commitments(&root, &auth));
        ChainData {
            epoch,
            summaries,
            raw_tx: p.tx,
            height: epoch.activation + b,
            block_header,
            block_txids,
            tip_header,
            tip_txids,
            tip_auth_commitments,
        }
    }

    fn inclusion_of(at: &Located) -> std::result::Result<seam_payment::Included, seam_payment::Error> {
        let txid = *at.transaction().unwrap().txid().as_ref();
        check_inclusion(
            &txid,
            &Inclusion {
                branch_id: at.epoch.branch_id,
                tx_index: at.tx_index as u32,
                txid_path: &at.txid_path,
                block_header: &at.block_header,
                leaf: &at.leaf,
                mmr_path: &at.mmr_path,
                peaks: &at.peaks,
                peak_index: at.peak_index as u32,
                tip_header: &at.tip_header,
                tip_auth_data_root: &at.tip_auth_data_root,
            },
        )
    }

    /// For every place of B in histories of 1 to 13 blocks, the located inclusion is one the statement accepts.
    #[test]
    fn a_located_transaction_is_included_under_t() {
        for n in 1..=13 {
            for b in 0..n {
                let d = chain_data(n, b);
                let at = assemble(ZcashNetwork::Testnet, &d).unwrap();
                let included = inclusion_of(&at).unwrap();
                assert_eq!(included.block_height, u64::from(d.height));
                assert_eq!(included.tip_hash, sha256d(&d.tip_header));
                assert_eq!((at.tx_index, at.block_transactions), (1, 3));
                assert_eq!((at.leaf_index, at.leaves), (b as usize, n as usize));
                assert_eq!(at.tip_height, d.epoch.activation + n);
                assert_eq!(at.peaks.len(), n.count_ones() as usize);
            }
        }
    }

    #[test]
    fn chain_data_that_does_not_fit_is_refused() {
        let ok = chain_data(6, 3);
        assert!(assemble(ZcashNetwork::Testnet, &ok).is_ok());
        assert!(assemble(ZcashNetwork::Mainnet, &ok).is_err(), "another network's epoch");
        let refused = |change: fn(&mut ChainData)| {
            let mut d = ok.clone();
            change(&mut d);
            format!("{:#}", assemble(ZcashNetwork::Testnet, &d).unwrap_err())
        };
        assert!(
            refused(|d| {
                d.summaries.remove(0);
            })
            .contains("activation")
        );
        assert!(refused(|d| d.summaries[2].height += 1).contains("consecutive"));
        assert!(refused(|d| d.height = d.epoch.activation + 6).contains("not below the anchor"));
        assert!(refused(|d| d.height -= 1).contains("not the summarized block"));
        assert!(refused(|d| d.block_txids.swap(0, 2)).contains("B's transactions"));
        assert!(refused(|d| d.block_txids[1] = [4; 32]).contains("B's transactions"));
        assert!(refused(|d| d.raw_tx.truncate(100)).contains("does not parse"));
        assert!(refused(|d| d.summaries[5].hash[0] ^= 1).contains("does not follow"));
        assert!(
            refused(|d| {
                d.tip_txids.pop();
            })
            .contains("T's transactions")
        );
        assert!(refused(|d| d.tip_auth_commitments.push([0; 32])).contains("auth data commitment"));
        assert!(refused(|d| d.tip_auth_commitments[0][0] ^= 1).contains("commits to"));
        assert!(refused(|d| d.summaries[1].ironwood_tx += 1).contains("commits to"));
    }

    #[test]
    fn data_files_are_named_by_network_and_epoch() {
        let epoch = ZcashNetwork::Testnet.epoch(4_488_560).unwrap();
        let files = DataFiles::in_dir(Path::new("data"), ZcashNetwork::Testnet, epoch);
        assert_eq!(files.headers, Path::new("data").join("testnet-77190ad9-4465026.headers.jsonl"));
        assert_eq!(files.summaries, Path::new("data").join("testnet-77190ad9-4465026.summaries.jsonl"));
    }
}
