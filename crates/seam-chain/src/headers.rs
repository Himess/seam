//! Header files: JSON Lines of `{"height", "hash" (display order), "raw" (hex)}`, the format Seam's
//! `contracts/test/relay/tools/zcash_p2p_headers.py` writes, extended from the peer-to-peer network ([`extend`]) and
//! read back with every hash and link checked ([`load`]). A header file holds public chain data only: unlike the
//! Python tool, nothing here writes the peers' addresses.

use std::{
    collections::{BTreeMap, VecDeque},
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Write},
    net::SocketAddr,
    path::Path,
};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::{
    history::{Header, display_hash, parse_display_hash, sha256d},
    network::ZcashNetwork,
    p2p::{PEER_HEADERS_PER_ANSWER, Peer},
};

/// How many of the file's last headers a locator names: a reorganisation of up to that many is followed.
pub const LOCATOR_DEPTH: usize = 32;

/// One line of a header file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeaderLine {
    pub height: u32,
    /// SHA-256d of the header, display order.
    pub hash: String,
    /// The serialized header, hex.
    pub raw: String,
}

impl HeaderLine {
    pub fn new(height: u32, raw: &[u8]) -> Self {
        Self {
            height,
            hash: display_hash(&sha256d(raw)),
            raw: hex::encode(raw),
        }
    }

    /// The header and its bytes, checked: a 1487-byte header whose SHA-256d is the line's hash.
    pub fn header(&self) -> Result<(Header, Vec<u8>)> {
        let raw = hex::decode(&self.raw).with_context(|| format!("header {}: not hex", self.height))?;
        let header = Header::parse(&raw).with_context(|| format!("header {}", self.height))?;
        ensure!(
            parse_display_hash(&self.hash).ok() == Some(header.hash),
            "header {}: its hash is not the line's",
            self.height
        );
        Ok((header, raw))
    }
}

/// Headers at consecutive heights, each linked to the one before.
#[derive(Clone, Debug)]
pub struct Headers {
    first: u32,
    headers: Vec<Header>,
}

impl Headers {
    pub fn get(&self, height: u32) -> Option<&Header> {
        self.headers.get(usize::try_from(height.checked_sub(self.first)?).ok()?)
    }

    pub fn first_height(&self) -> u32 {
        self.first
    }

    pub fn last_height(&self) -> u32 {
        self.first + self.headers.len() as u32 - 1
    }

    pub fn len(&self) -> usize {
        self.headers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.headers.is_empty()
    }
}

/// What [`load`] reads from a header file.
pub struct Loaded {
    /// The headers at the heights asked for.
    pub headers: Headers,
    /// The serialized headers at the heights asked for raw.
    pub raw: BTreeMap<u32, Vec<u8>>,
}

/// Reads the headers at heights `from..=to` from header file `path`, and the bytes of those at `raw` (each in that
/// range). Every line up to `to` is checked: its hash, its height (one more than the line before) and its link to
/// the header before.
pub fn load(path: &Path, from: u32, to: u32, raw: &[u32]) -> Result<Loaded> {
    ensure!(from <= to, "no heights to read");
    let file = File::open(path).with_context(|| format!("open the header file {}", path.display()))?;
    let mut prev: Option<(u32, [u8; 32])> = None;
    let mut headers = Vec::with_capacity((to - from + 1) as usize);
    let mut raws = BTreeMap::new();
    for (n, line) in BufReader::new(file).lines().enumerate() {
        let line = line.with_context(|| format!("read the header file {}", path.display()))?;
        if line.trim().is_empty() {
            continue;
        }
        let l: HeaderLine =
            serde_json::from_str(&line).with_context(|| format!("line {} of the header file", n + 1))?;
        if l.height > to {
            break;
        }
        let (header, bytes) = l.header()?;
        if let Some((height, hash)) = prev {
            ensure!(
                l.height == height + 1 && header.prev_hash == hash,
                "the header file is not one chain at height {}",
                l.height
            );
        }
        prev = Some((l.height, header.hash));
        if l.height >= from {
            headers.push(header);
        }
        if raw.contains(&l.height) {
            raws.insert(l.height, bytes);
        }
    }
    let first = prev.map(|(last, _)| last + 1 - headers.len() as u32);
    ensure!(
        !headers.is_empty() && first == Some(from) && prev.map(|(h, _)| h) == Some(to),
        "the header file does not hold every header from {from} to {to}"
    );
    for h in raw {
        ensure!(raws.contains_key(h), "the header file does not hold header {h}");
    }
    Ok(Loaded {
        headers: Headers { first: from, headers },
        raw: raws,
    })
}

/// A header at the end of the chain a file holds: its height, its hash and where its line starts (`None` for the
/// block before a new file's first header, which the file does not hold).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Tip {
    height: u32,
    hash: [u8; 32],
    offset: Option<u64>,
}

/// The last [`LOCATOR_DEPTH`] headers of header file `path`, oldest first; empty if it does not exist or is empty.
fn read_tail(path: &Path) -> Result<VecDeque<Tip>> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(VecDeque::new()),
        Err(e) => return Err(e).with_context(|| format!("open the header file {}", path.display())),
    };
    let mut reader = BufReader::new(file);
    let mut tail = VecDeque::with_capacity(LOCATOR_DEPTH + 1);
    let (mut offset, mut line) = (0u64, String::new());
    loop {
        line.clear();
        let n = reader.read_line(&mut line).with_context(|| format!("read the header file {}", path.display()))?;
        if n == 0 {
            break;
        }
        if !line.trim().is_empty() {
            let l: HeaderLine = serde_json::from_str(line.trim()).context("a line of the header file")?;
            let hash = parse_display_hash(&l.hash)?;
            if let Some(last) = tail.back() {
                let last: &Tip = last;
                ensure!(l.height == last.height + 1, "the header file skips from {} to {}", last.height, l.height);
            }
            tail.push_back(Tip {
                height: l.height,
                hash,
                offset: Some(offset),
            });
            if tail.len() > LOCATOR_DEPTH {
                tail.pop_front();
            }
        }
        offset += n as u64;
    }
    Ok(tail)
}

/// How a peer's answer to a locator of `tail` continues our chain.
#[derive(Debug, PartialEq, Eq)]
enum Answer {
    /// Its headers follow our last one.
    Extends,
    /// They follow our header at this height, below our last one, and reach higher than our chain: a reorganisation.
    Reorganizes(u32),
    /// They follow none of our headers, are not linked to each other, or follow an earlier one of ours without
    /// reaching higher than our chain: another peer is asked.
    Unusable,
}

fn judge(tail: &VecDeque<Tip>, raws: &[Vec<u8>]) -> Answer {
    let (Some(first), Some(last)) = (raws.first(), tail.back()) else {
        return Answer::Unusable;
    };
    if raws.windows(2).any(|pair| pair[1][4..36] != sha256d(&pair[0])) {
        return Answer::Unusable;
    }
    match tail.iter().rev().find(|t| first[4..36] == t.hash) {
        Some(t) if t.height == last.height => Answer::Extends,
        // A header the file does not hold cannot be cut back to.
        Some(t) if t.offset.is_some() && t.height as usize + raws.len() > last.height as usize => {
            Answer::Reorganizes(t.height)
        }
        _ => Answer::Unusable,
    }
}

/// Extends header file `path` with the headers of Zcash `network` up to height `until`, or to the peers' tip if it is
/// lower, and returns the file's last height. A file that does not exist (or is empty) is started after `start`, the
/// height and hash (internal byte order) of the block before its first header. A tail the peers' best chain no longer
/// holds (a reorganisation of up to [`LOCATOR_DEPTH`] headers) is replaced by theirs.
///
/// `peers` are tried in turn; the one that answered last is moved first, for the next call.
pub fn extend(
    network: ZcashNetwork,
    path: &Path,
    start: Option<(u32, [u8; 32])>,
    until: u32,
    peers: &mut Vec<SocketAddr>,
) -> Result<u32> {
    let mut tail = read_tail(path)?;
    if tail.is_empty() {
        let (height, hash) = start.context("the header file is empty and no block to start after is given")?;
        tail.push_back(Tip {
            height,
            hash,
            offset: None,
        });
    }
    let mut last = tail.back().expect("non-empty").height;
    if last >= until {
        return Ok(last);
    }
    let mut candidates = peers.clone().into_iter();
    let mut peer: Option<(SocketAddr, Peer)> = None;
    let mut answered = None;
    while last < until {
        let (addr, p) = match peer.as_mut() {
            Some((a, p)) => (*a, p),
            None => {
                let Some(addr) = candidates.next() else { break };
                match Peer::connect(network, addr) {
                    Ok(p) => {
                        let (_, p) = peer.insert((addr, p));
                        (addr, p)
                    }
                    Err(_) => continue,
                }
            }
        };
        let locator: Vec<[u8; 32]> = tail.iter().rev().map(|t| t.hash).collect();
        let raws = match p.get_headers(&locator) {
            Ok(r) => r,
            Err(_) => {
                peer = None;
                continue;
            }
        };
        if raws.is_empty() {
            if p.start_height > last {
                // It announced more blocks, but none after ours: it is on another branch.
                peer = None;
                continue;
            }
            answered = Some(addr);
            break; // the peer's tip
        }
        match judge(&tail, &raws) {
            Answer::Extends => {}
            Answer::Reorganizes(height) => {
                let cut = tail.iter().find(|t| t.height == height + 1).and_then(|t| t.offset).context("cut point")?;
                OpenOptions::new()
                    .write(true)
                    .open(path)
                    .and_then(|f| f.set_len(cut))
                    .with_context(|| format!("cut the header file {} back", path.display()))?;
                tail.retain(|t| t.height <= height);
                last = height;
            }
            Answer::Unusable => {
                peer = None;
                continue;
            }
        }
        answered = Some(addr);
        let at_tip = raws.len() < PEER_HEADERS_PER_ANSWER;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("open the header file {}", path.display()))?;
        let mut offset = file.metadata()?.len();
        let mut lines = String::new();
        for raw in raws.iter().take((until - last) as usize) {
            last += 1;
            let line = serde_json::to_string(&HeaderLine::new(last, raw))? + "\n";
            tail.push_back(Tip {
                height: last,
                hash: sha256d(raw),
                offset: Some(offset),
            });
            if tail.len() > LOCATOR_DEPTH {
                tail.pop_front();
            }
            offset += line.len() as u64;
            lines.push_str(&line);
        }
        file.write_all(lines.as_bytes())
            .with_context(|| format!("write the header file {}", path.display()))?;
        if at_tip {
            break;
        }
    }
    let Some(addr) = answered else {
        bail!("no Zcash {network} peer answered (tried {} addresses)", peers.len());
    };
    peers.retain(|a| *a != addr);
    peers.insert(0, addr);
    Ok(last)
}

/// The height of the last header of header file `path`, if it holds any.
pub fn last_height(path: &Path) -> Result<Option<u32>> {
    Ok(read_tail(path)?.back().map(|t| t.height))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A synthetic header (not a valid block: only its length, its link and its hash matter here) after `prev`, with
    /// `tag` in its nonce so that branches differ.
    pub(crate) fn synthetic_header(prev: [u8; 32], tag: u8) -> Vec<u8> {
        let mut raw = vec![0u8; 1487];
        raw[0] = 4;
        raw[4..36].copy_from_slice(&prev);
        raw[100..104].copy_from_slice(&1_700_000_000u32.to_le_bytes());
        raw[104..108].copy_from_slice(&0x1f07_ffffu32.to_le_bytes());
        raw[108] = tag;
        raw[140..143].copy_from_slice(&[0xfd, 0x40, 0x05]);
        raw
    }

    /// `n` linked headers after `prev`.
    pub(crate) fn chain(prev: [u8; 32], n: usize, tag: u8) -> Vec<Vec<u8>> {
        let mut out: Vec<Vec<u8>> = vec![];
        let mut prev = prev;
        for _ in 0..n {
            let raw = synthetic_header(prev, tag);
            prev = sha256d(&raw);
            out.push(raw);
        }
        out
    }

    pub(crate) fn write_file(path: &Path, first: u32, raws: &[Vec<u8>]) {
        let mut text = String::new();
        for (i, raw) in raws.iter().enumerate() {
            text.push_str(&serde_json::to_string(&HeaderLine::new(first + i as u32, raw)).unwrap());
            text.push('\n');
        }
        std::fs::write(path, text).unwrap();
    }

    pub(crate) fn temp_path(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("seam-chain-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn a_header_file_is_read_back_checked() {
        let path = temp_path("headers.jsonl");
        let raws = chain([7; 32], 10, 0);
        write_file(&path, 100, &raws);
        let loaded = load(&path, 103, 108, &[103, 108]).unwrap();
        assert_eq!((loaded.headers.first_height(), loaded.headers.last_height()), (103, 108));
        assert_eq!(loaded.headers.get(105).unwrap().hash, sha256d(&raws[5]));
        assert!(loaded.headers.get(102).is_none() && loaded.headers.get(109).is_none());
        assert_eq!(loaded.raw[&108], raws[8]);
        assert!(load(&path, 103, 110, &[]).is_err(), "beyond the file");
        assert!(load(&path, 99, 105, &[]).is_err(), "before the file");
        assert!(load(&path, 103, 105, &[106]).is_err(), "a raw header outside the range");
        assert_eq!(last_height(&path).unwrap(), Some(109));

        // A line whose hash is not its header's, and a broken link, are refused.
        let mut lines: Vec<HeaderLine> =
            raws.iter().enumerate().map(|(i, r)| HeaderLine::new(100 + i as u32, r)).collect();
        lines[4].hash = lines[5].hash.clone();
        let text: String = lines.iter().map(|l| serde_json::to_string(l).unwrap() + "\n").collect();
        std::fs::write(&path, text).unwrap();
        assert!(load(&path, 100, 109, &[]).is_err());
        let mut broken = raws.clone();
        broken[6] = synthetic_header([9; 32], 0);
        write_file(&path, 100, &broken);
        assert!(load(&path, 100, 105, &[]).is_ok(), "the lines before the break are one chain");
        assert!(load(&path, 100, 109, &[]).is_err());
        std::fs::remove_file(&path).unwrap();
        assert_eq!(last_height(&path).unwrap(), None);
    }

    #[test]
    fn answers_are_judged_against_the_file_tail() {
        let path = temp_path("tail.jsonl");
        let ours = chain([7; 32], 40, 0);
        write_file(&path, 1, &ours);
        let tail = read_tail(&path).unwrap();
        assert_eq!(tail.len(), LOCATOR_DEPTH);
        assert_eq!(tail.back().unwrap().height, 40);
        assert_eq!(tail.front().unwrap().height, 40 - LOCATOR_DEPTH as u32 + 1);

        // Headers after our last one extend the chain.
        let next = chain(sha256d(&ours[39]), 5, 0);
        assert_eq!(judge(&tail, &next), Answer::Extends);
        // A branch from our header 37 (index 36) that reaches higher than our 40 is a reorganisation; one that does
        // not, or that forks below the locator, or is not linked, is not used.
        let longer = chain(sha256d(&ours[36]), 4, 1);
        assert_eq!(judge(&tail, &longer), Answer::Reorganizes(37));
        let shorter = chain(sha256d(&ours[36]), 3, 1);
        assert_eq!(judge(&tail, &shorter), Answer::Unusable);
        let deep = chain(sha256d(&ours[2]), 100, 1);
        assert_eq!(judge(&tail, &deep), Answer::Unusable);
        let mut unlinked = next.clone();
        unlinked.swap(1, 2);
        assert_eq!(judge(&tail, &unlinked), Answer::Unusable);
        assert_eq!(judge(&tail, &chain([1; 32], 3, 0)), Answer::Unusable);
        assert_eq!(judge(&tail, &[]), Answer::Unusable);

        // The cut point of a reorganisation at 37 is the start of line 38: the file then ends at 37.
        let cut = tail.iter().find(|t| t.height == 38).unwrap().offset.unwrap();
        OpenOptions::new().write(true).open(&path).unwrap().set_len(cut).unwrap();
        assert_eq!(last_height(&path).unwrap(), Some(37));
        assert_eq!(load(&path, 1, 37, &[]).unwrap().headers.len(), 37);

        // A new file's starting block cannot be cut back to.
        let start = VecDeque::from([Tip {
            height: 9,
            hash: [3; 32],
            offset: None,
        }]);
        assert_eq!(judge(&start, &chain([3; 32], 2, 0)), Answer::Extends);
        std::fs::remove_file(&path).unwrap();
    }
}
