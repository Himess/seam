//! Per-block summaries, what a history-tree leaf records about each block: rebuilt from a light wallet server's
//! compact blocks (every pool type) and the P2P headers, and kept in a cache. Ported from Zecured's
//! `signer/crates/deposit-witness/src/chain.rs` and `witness.rs` (73de9f909e270e8aea10c2b221905f62a6a4c4df).
//!
//! The three note commitment trees restart from the server's tree states and are rebuilt block by block; at the end
//! of every chunk of blocks they must equal the server's tree states there, and every block's hash must be its P2P
//! header's. The cache (JSON Lines of [`BlockSummary`], public chain data) holds only summaries checked that way, and
//! is cut back where a summary no longer matches the headers (a reorganisation).

use std::{
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::Path,
};

use anyhow::{Context, Result, anyhow, ensure};
use incrementalmerkletree::frontier::CommitmentTree;
use orchard::tree::MerkleHashOrchard;
use zcash_client_backend::proto::{compact_formats::CompactBlock, service::TreeState};

use crate::{
    headers::Headers,
    history::{BlockSummary, Header},
    lwd::{self, LwdClient},
    network::Epoch,
};

/// Blocks per compact block stream, and between two checks against the server's tree states.
pub const CHUNK: u32 = 1_000;

/// The Sapling, Orchard and Ironwood note commitment trees, advanced block by block.
pub struct Trees {
    sapling: CommitmentTree<sapling::Node, { sapling::NOTE_COMMITMENT_TREE_DEPTH }>,
    orchard: CommitmentTree<MerkleHashOrchard, { orchard::NOTE_COMMITMENT_TREE_DEPTH as u8 }>,
    ironwood: CommitmentTree<MerkleHashOrchard, { orchard::NOTE_COMMITMENT_TREE_DEPTH as u8 }>,
}

impl Trees {
    /// The trees of a server's tree state.
    pub fn from_state(state: &TreeState) -> Result<Self> {
        Ok(Self {
            sapling: state.sapling_tree().context("the Sapling tree state")?,
            orchard: state.orchard_tree().context("the Orchard tree state")?,
            ironwood: state.ironwood_tree().context("the Ironwood tree state")?,
        })
    }

    /// The three roots (LEBS2OSP_256), as a history-tree leaf records them.
    pub fn roots(&self) -> [[u8; 32]; 3] {
        [
            self.sapling.root().to_bytes(),
            self.orchard.root().to_bytes(),
            self.ironwood.root().to_bytes(),
        ]
    }
}

/// The summary of `block`, whose header is `header`, advancing `trees` past it.
pub fn summarize(block: &CompactBlock, header: &Header, trees: &mut Trees) -> Result<BlockSummary> {
    let height = u32::try_from(block.height)?;
    ensure!(block.hash == header.hash, "compact block {height} is not the block of its header");
    ensure!(
        block.vtx.iter().enumerate().all(|(i, t)| t.index == i as u64),
        "the server left transactions of block {height} out"
    );
    let (mut sapling_tx, mut orchard_tx, mut ironwood_tx) = (0u64, 0u64, 0u64);
    for tx in &block.vtx {
        if !tx.spends.is_empty() || !tx.outputs.is_empty() {
            sapling_tx += 1;
        }
        if !tx.actions.is_empty() {
            orchard_tx += 1;
        }
        if !tx.ironwood_actions.is_empty() {
            ironwood_tx += 1;
        }
        for o in &tx.outputs {
            let cmu = o.cmu().map_err(|_| anyhow!("block {height}: a Sapling note commitment that is not one"))?;
            trees
                .sapling
                .append(sapling::Node::from_cmu(&cmu))
                .map_err(|_| anyhow!("the Sapling tree is full"))?;
        }
        for a in &tx.actions {
            let cmx = a.cmx().map_err(|_| anyhow!("block {height}: an Orchard note commitment that is not one"))?;
            trees
                .orchard
                .append(MerkleHashOrchard::from_cmx(&cmx))
                .map_err(|_| anyhow!("the Orchard tree is full"))?;
        }
        for a in &tx.ironwood_actions {
            let cmx = a.cmx().map_err(|_| anyhow!("block {height}: an Ironwood note commitment that is not one"))?;
            trees
                .ironwood
                .append(MerkleHashOrchard::from_cmx(&cmx))
                .map_err(|_| anyhow!("the Ironwood tree is full"))?;
        }
    }
    let [sapling_root, orchard_root, ironwood_root] = trees.roots();
    Ok(BlockSummary {
        height,
        hash: header.hash,
        time: header.time,
        bits: header.bits,
        sapling_root,
        orchard_root,
        ironwood_root,
        sapling_tx,
        orchard_tx,
        ironwood_tx,
    })
}

/// That a summary's roots are those of the server's tree state after its block.
pub fn check_against_state(summary: &BlockSummary, state: &TreeState) -> Result<()> {
    let roots = Trees::from_state(state)?.roots();
    ensure!(
        roots == [summary.sapling_root, summary.orchard_root, summary.ironwood_root],
        "the note commitment trees rebuilt up to block {} are not the server's",
        summary.height
    );
    Ok(())
}

/// Summaries of blocks `from..=to` (all in `headers`), rebuilt from the server's tree state after block `from - 1`
/// and checked against its tree state at the end of every chunk. `chunk_done` gets each checked chunk.
pub async fn block_summaries(
    client: &mut LwdClient,
    headers: &Headers,
    from: u32,
    to: u32,
    chunk_done: &mut dyn FnMut(&[BlockSummary]) -> Result<()>,
) -> Result<Vec<BlockSummary>> {
    ensure!(from >= 1 && from <= to, "no blocks to summarize");
    let mut trees = Trees::from_state(&lwd::tree_state(client, from - 1).await?)?;
    let mut out = Vec::with_capacity((to - from + 1) as usize);
    let mut start = from;
    while start <= to {
        let end = start.saturating_add(CHUNK - 1).min(to);
        let mut chunk = Vec::with_capacity((end - start + 1) as usize);
        for block in lwd::compact_blocks(client, start, end).await? {
            let height = u32::try_from(block.height)?;
            let header = headers.get(height).with_context(|| format!("no header for block {height}"))?;
            chunk.push(summarize(&block, header, &mut trees)?);
        }
        check_against_state(chunk.last().expect("a chunk has blocks"), &lwd::tree_state(client, end).await?)?;
        chunk_done(&chunk)?;
        out.extend(chunk);
        start = end + 1;
    }
    Ok(out)
}

/// The summaries of a cache file at `path` that start at `epoch`'s activation and match `headers` (where it holds
/// the block), and the byte length of the file's part they span. A cache of another epoch, or one that no longer
/// matches, is used up to where it does.
fn read_cache(path: &Path, epoch: Epoch, headers: &Headers) -> Result<(Vec<BlockSummary>, u64)> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((vec![], 0)),
        Err(e) => return Err(e).with_context(|| format!("open the summary cache {}", path.display())),
    };
    let mut reader = BufReader::new(file);
    let (mut out, mut good_len, mut line) = (Vec::<BlockSummary>::new(), 0u64, String::new());
    loop {
        line.clear();
        let n = reader.read_line(&mut line).with_context(|| format!("read the summary cache {}", path.display()))?;
        if n == 0 || !line.ends_with('\n') {
            break; // the end, or a line cut short
        }
        let Ok(s) = serde_json::from_str::<BlockSummary>(line.trim()) else {
            break;
        };
        let expected = epoch.activation + out.len() as u32;
        if s.height != expected || headers.get(s.height).is_some_and(|h| h.hash != s.hash) {
            break;
        }
        out.push(s);
        good_len += n as u64;
    }
    Ok((out, good_len))
}

/// The summaries of `epoch`'s blocks up to `last` (all in `headers`): those of the cache at `path`, then the rest
/// rebuilt from the server, appended to the cache chunk by chunk (the first build of a long epoch takes minutes,
/// later ones seconds). `progress` gets the height each checked chunk reaches.
pub async fn summaries(
    client: &mut LwdClient,
    headers: &Headers,
    path: &Path,
    epoch: Epoch,
    last: u32,
    progress: &mut dyn FnMut(u32),
) -> Result<Vec<BlockSummary>> {
    ensure!(epoch.contains(last), "block {last} is not in the epoch");
    let (mut out, good_len) = read_cache(path, epoch, headers)?;
    // Keep what matches; drop the rest of the file (another epoch's cache, a reorganised block, a cut line). An
    // append-only handle may not truncate (Windows), so the cut has its own.
    if std::fs::metadata(path).is_ok_and(|m| m.len() != good_len) {
        OpenOptions::new()
            .write(true)
            .open(path)
            .and_then(|f| f.set_len(good_len))
            .with_context(|| format!("cut the summary cache {} back", path.display()))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open the summary cache {}", path.display()))?;
    out.truncate((last + 1 - epoch.activation) as usize);
    let have = out.last().map_or(epoch.activation - 1, |s| s.height);
    if have < last {
        // The server's trees after the last cached block must be the cache's.
        if let Some(s) = out.last() {
            check_against_state(s, &lwd::tree_state(client, have).await?)
                .context("the summary cache is not the light wallet server's chain: delete it")?;
        }
        let mut append = |chunk: &[BlockSummary]| -> Result<()> {
            let mut text = String::new();
            for s in chunk {
                text.push_str(&serde_json::to_string(s)?);
                text.push('\n');
            }
            file.write_all(text.as_bytes())
                .with_context(|| format!("write the summary cache {}", path.display()))?;
            progress(chunk.last().expect("a chunk has blocks").height);
            Ok(())
        };
        out.extend(block_summaries(client, headers, have + 1, last, &mut append).await?);
    }
    ensure!(out.len() == (last + 1 - epoch.activation) as usize, "the summaries have a gap");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use zcash_client_backend::proto::compact_formats::{CompactOrchardAction, CompactSaplingOutput, CompactTx};

    use super::*;

    fn empty_trees() -> Trees {
        Trees::from_state(&TreeState::default()).unwrap()
    }

    fn header(hash: [u8; 32]) -> Header {
        Header {
            hash,
            prev_hash: [0; 32],
            merkle_root: [0; 32],
            block_commitments: [0; 32],
            time: 1_700_000_000,
            bits: 0x1f07_ffff,
        }
    }

    /// A real Ironwood note commitment: the first action's cmx of the fixture's transaction.
    fn real_cmx() -> Vec<u8> {
        use zcash_primitives::transaction::Transaction;
        let p: seam_payment::k4::PublicTx = serde_json::from_slice(
            &std::fs::read(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../seam-payment/tests/fixtures/TEST-ONLY_testnet_ironwood_tx.json"
            ))
            .unwrap(),
        )
        .unwrap();
        let tx = Transaction::read(&p.tx[..], zcash_protocol::consensus::BranchId::Nu7).unwrap();
        tx.ironwood_bundle().unwrap().actions()[0].cmx().to_bytes().to_vec()
    }

    fn action(cmx: &[u8]) -> CompactOrchardAction {
        CompactOrchardAction {
            cmx: cmx.to_vec(),
            ..Default::default()
        }
    }

    /// A block's summary counts the transactions of each pool and advances each tree by its notes alone.
    #[test]
    fn a_block_is_summarized() {
        let cmx = real_cmx();
        let block = CompactBlock {
            height: 10,
            hash: vec![5; 32],
            vtx: vec![
                CompactTx {
                    index: 0,
                    ..Default::default()
                },
                CompactTx {
                    index: 1,
                    outputs: vec![CompactSaplingOutput {
                        cmu: vec![0; 32],
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                CompactTx {
                    index: 2,
                    ironwood_actions: vec![action(&cmx), action(&cmx)],
                    ..Default::default()
                },
                CompactTx {
                    index: 3,
                    actions: vec![action(&cmx)],
                    ironwood_actions: vec![action(&cmx)],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let mut trees = empty_trees();
        let empty = trees.roots();
        let s = summarize(&block, &header([5; 32]), &mut trees).unwrap();
        assert_eq!((s.height, s.hash, s.time, s.bits), (10, [5; 32], 1_700_000_000, 0x1f07_ffff));
        assert_eq!((s.sapling_tx, s.orchard_tx, s.ironwood_tx), (1, 1, 2));
        assert_eq!([s.sapling_root, s.orchard_root, s.ironwood_root], trees.roots());
        assert!(s.sapling_root != empty[0] && s.orchard_root != empty[1] && s.ironwood_root != empty[2]);
        // The same notes in another tree give the same root only with the same count.
        let mut ironwood_only = empty_trees();
        for _ in 0..3 {
            ironwood_only.ironwood.append(MerkleHashOrchard::from_cmx(&tx_cmx(&cmx))).unwrap();
        }
        assert_eq!(ironwood_only.roots()[2], s.ironwood_root);
        assert_ne!(ironwood_only.roots()[1], s.orchard_root);

        // Another block's header, a transaction left out, a note commitment that is not one: refused.
        assert!(summarize(&block, &header([6; 32]), &mut empty_trees()).is_err());
        let mut gap = block.clone();
        gap.vtx.remove(1);
        assert!(summarize(&gap, &header([5; 32]), &mut empty_trees()).is_err());
        let mut bad = block.clone();
        bad.vtx[2].ironwood_actions[0].cmx = vec![0xff; 32];
        assert!(summarize(&bad, &header([5; 32]), &mut empty_trees()).is_err());
    }

    fn tx_cmx(bytes: &[u8]) -> orchard::note::ExtractedNoteCommitment {
        orchard::note::ExtractedNoteCommitment::from_bytes(&bytes.try_into().unwrap()).unwrap()
    }

    /// A cache is used from the epoch's first block for as long as it matches the headers, and not past a cut line.
    #[test]
    fn a_cache_is_used_as_far_as_it_matches() {
        use crate::headers::tests::{chain, temp_path, write_file};
        let epoch = Epoch {
            branch_id: seam_payment::NU7_BRANCH_ID,
            activation: 100,
            next_activation: None,
        };
        let raws = chain([1; 32], 10, 0);
        let headers_path = temp_path("cache-headers.jsonl");
        write_file(&headers_path, 100, &raws);
        let headers = crate::headers::load(&headers_path, 100, 109, &[]).unwrap().headers;
        let summary = |i: usize, hash: [u8; 32]| BlockSummary {
            height: 100 + i as u32,
            hash,
            time: 0,
            bits: 0,
            sapling_root: [0; 32],
            orchard_root: [0; 32],
            ironwood_root: [0; 32],
            sapling_tx: 0,
            orchard_tx: 0,
            ironwood_tx: 0,
        };
        let lines =
            |ss: &[BlockSummary]| -> String { ss.iter().map(|s| serde_json::to_string(s).unwrap() + "\n").collect() };
        let good: Vec<BlockSummary> = (0..8).map(|i| summary(i, headers.get(100 + i as u32).unwrap().hash)).collect();
        let path = temp_path("cache.jsonl");

        std::fs::write(&path, lines(&good)).unwrap();
        let (read, len) = read_cache(&path, epoch, &headers).unwrap();
        assert_eq!((read.len(), len), (8, std::fs::metadata(&path).unwrap().len()));

        // A summary whose block is not the header's: used up to it.
        let mut stale = good.clone();
        stale[5].hash = [9; 32];
        std::fs::write(&path, lines(&stale)).unwrap();
        let (read, len) = read_cache(&path, epoch, &headers).unwrap();
        assert_eq!((read.len(), len), (5, lines(&good[..5]).len() as u64));

        // A line cut short, a gap, another epoch's cache.
        let text = lines(&good);
        std::fs::write(&path, &text[..text.len() - 3]).unwrap();
        assert_eq!(read_cache(&path, epoch, &headers).unwrap().0.len(), 7);
        let mut gap = good.clone();
        gap.remove(3);
        std::fs::write(&path, lines(&gap)).unwrap();
        assert_eq!(read_cache(&path, epoch, &headers).unwrap().0.len(), 3);
        let other = Epoch {
            activation: 101,
            ..epoch
        };
        std::fs::write(&path, lines(&good)).unwrap();
        assert_eq!(read_cache(&path, other, &headers).unwrap(), (vec![], 0));
        std::fs::remove_file(&path).unwrap();
        assert_eq!(read_cache(&path, epoch, &headers).unwrap(), (vec![], 0));
        std::fs::remove_file(&headers_path).unwrap();
    }
}
