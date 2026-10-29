//! The ZIP 221 chain history tree (V3 nodes, ZIP 258) of an epoch, and the two Merkle trees of a block that a payment
//! proof walks: its transactions' (`hashMerkleRoot`) and their authorizing data's (`hashAuthDataRoot`, ZIP 244).
//! Ported from Zecured's `signer/crates/deposit-witness/src/history.rs` (73de9f909e270e8aea10c2b221905f62a6a4c4df),
//! whose history roots matched the commitments of real Testnet headers.
//!
//! The header of block n commits, through `hashBlockCommitments`, to a Merkle mountain range with one leaf per earlier
//! block of its network-upgrade epoch (from the epoch's activation height to n - 1). Each leaf records the block's
//! hash, time, difficulty and work, the final Sapling, Orchard and Ironwood note commitment tree roots after it, and
//! how many of its transactions use each of those pools.

use anyhow::{Result, anyhow, ensure};
use primitive_types::U256;
use serde::{Deserialize, Serialize};
use zcash_history::{Entry, NodeData, NodeDataV2, NodeDataV3, Tree, V3, Version};

pub use seam_payment::{HEADER_LENGTH, block_commitments, sha256d};

/// The fields of a block header the history tree and a payment witness need.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    /// SHA-256d of the serialized header, internal byte order.
    pub hash: [u8; 32],
    pub prev_hash: [u8; 32],
    pub merkle_root: [u8; 32],
    pub block_commitments: [u8; 32],
    pub time: u32,
    pub bits: u32,
}

impl Header {
    /// Parses a serialized post-NU5 header (140-byte prefix, compactSize, 1344-byte Equihash solution).
    pub fn parse(raw: &[u8]) -> Result<Self> {
        ensure!(raw.len() == HEADER_LENGTH, "a header is {} bytes, not {HEADER_LENGTH}", raw.len());
        let field = |at: usize| -> [u8; 32] { raw[at..at + 32].try_into().expect("32 bytes") };
        let word = |at: usize| u32::from_le_bytes(raw[at..at + 4].try_into().expect("4 bytes"));
        Ok(Self {
            hash: sha256d(raw),
            prev_hash: field(4),
            merkle_root: field(36),
            block_commitments: field(68),
            time: word(100),
            bits: word(104),
        })
    }
}

/// A hash as block explorers and header files show it: reversed, in hex.
pub fn display_hash(hash: &[u8; 32]) -> String {
    let mut h = *hash;
    h.reverse();
    hex::encode(h)
}

/// [`display_hash`] read back into internal byte order.
pub fn parse_display_hash(s: &str) -> Result<[u8; 32]> {
    let mut h: [u8; 32] = hex::decode(s)?.try_into().map_err(|_| anyhow!("a hash is 32 bytes"))?;
    h.reverse();
    Ok(h)
}

/// Serde for a block hash in display order (as header files and block explorers write it).
mod display_order {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(hash: &[u8; 32], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&super::display_hash(hash))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 32], D::Error> {
        super::parse_display_hash(&String::deserialize(d)?).map_err(D::Error::custom)
    }
}

/// What a history-tree leaf records about one block.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockSummary {
    pub height: u32,
    /// The block hash (serialized in display order).
    #[serde(with = "display_order")]
    pub hash: [u8; 32],
    pub time: u32,
    pub bits: u32,
    /// The final note commitment tree roots after the block (LEBS2OSP_256 of each root).
    #[serde(with = "hex::serde")]
    pub sapling_root: [u8; 32],
    #[serde(with = "hex::serde")]
    pub orchard_root: [u8; 32],
    #[serde(with = "hex::serde")]
    pub ironwood_root: [u8; 32],
    /// Transactions with Sapling spends or outputs, with Orchard-pool actions, with Ironwood actions.
    pub sapling_tx: u64,
    pub orchard_tx: u64,
    pub ironwood_tx: u64,
}

/// The target a compact `nBits` value encodes, or `None` if it is negative or overflows 256 bits (Bitcoin's
/// `SetCompact`, which zcashd's `GetBlockProof` uses).
pub fn target_from_bits(bits: u32) -> Option<U256> {
    let exponent = bits >> 24;
    let mantissa = bits & 0x007f_ffff;
    let negative = mantissa != 0 && bits & 0x0080_0000 != 0;
    let overflow =
        mantissa != 0 && (exponent > 34 || (mantissa > 0xff && exponent > 33) || (mantissa > 0xffff && exponent > 32));
    if negative || overflow {
        return None;
    }
    let mantissa = U256::from(mantissa);
    Some(if exponent <= 3 {
        mantissa >> (8 * (3 - exponent))
    } else {
        mantissa << (8 * (exponent - 3))
    })
}

/// The work of a block: floor(2^256 / (target + 1)), as zcashd's `GetBlockProof`, and 0 for a target it refuses.
pub fn block_work(bits: u32) -> U256 {
    match target_from_bits(bits) {
        // 2^256 / (t + 1) = (2^256 - t - 1) / (t + 1) + 1, and 2^256 - t - 1 = !t. A target is never 2^256 - 1 (its
        // mantissa is at most 0x7fffff), so t + 1 does not overflow.
        Some(target) if !target.is_zero() => (!target / (target + U256::one())) + U256::one(),
        _ => U256::zero(),
    }
}

/// The ZIP 221 V3 leaf of a block of the epoch whose consensus branch id is `branch_id`.
pub fn leaf(branch_id: u32, b: &BlockSummary) -> NodeDataV3 {
    NodeDataV3 {
        v2: NodeDataV2 {
            v1: NodeData {
                consensus_branch_id: branch_id,
                subtree_commitment: b.hash,
                start_time: b.time,
                end_time: b.time,
                start_target: b.bits,
                end_target: b.bits,
                start_sapling_root: b.sapling_root,
                end_sapling_root: b.sapling_root,
                subtree_total_work: block_work(b.bits),
                start_height: u64::from(b.height),
                end_height: u64::from(b.height),
                sapling_tx: b.sapling_tx,
            },
            start_orchard_root: b.orchard_root,
            end_orchard_root: b.orchard_root,
            orchard_tx: b.orchard_tx,
        },
        start_ironwood_root: b.ironwood_root,
        end_ironwood_root: b.ironwood_root,
        ironwood_tx: b.ironwood_tx,
    }
}

/// `hashChainHistoryRoot` of the history tree over `leaves` (oldest first), as zcash_history's incremental tree builds
/// it: the BLAKE2b-256 digest of the serialized root node (ZIP 221).
pub fn history_root(leaves: &[NodeDataV3]) -> Result<[u8; 32]> {
    let (first, rest) = leaves.split_first().ok_or_else(|| anyhow!("an empty history"))?;
    let mut tree = Tree::<V3>::new(1, vec![(0, Entry::new_leaf(first.clone()))], vec![]);
    for l in rest {
        tree.append_leaf(l.clone()).map_err(|e| anyhow!("append a history leaf: {e}"))?;
    }
    let root = tree.root_node().map_err(|e| anyhow!("the history root: {e}"))?;
    Ok(V3::hash(root.data()))
}

fn blake2b_256(personal: &[u8; 16], parts: &[&[u8]]) -> [u8; 32] {
    let mut state = blake2b_simd::Params::new().hash_length(32).personal(personal).to_state();
    for p in parts {
        state.update(p);
    }
    state.finalize().as_bytes().try_into().expect("32 bytes")
}

/// `hashAuthDataRoot` (ZIP 244): a binary Merkle tree over the transactions' authorizing data commitments, in block
/// order, padded with zero leaves to a power of two.
pub fn auth_data_root(auth_commitments: &[[u8; 32]]) -> Result<[u8; 32]> {
    ensure!(!auth_commitments.is_empty(), "a block has at least its coinbase transaction");
    let mut level = auth_commitments.to_vec();
    level.resize(level.len().next_power_of_two(), [0u8; 32]);
    while level.len() > 1 {
        level = level.chunks(2).map(|p| blake2b_256(b"ZcashAuthDatHash", &[&p[0], &p[1]])).collect();
    }
    Ok(level[0])
}

/// A block's txid Merkle root (`hashMerkleRoot`): SHA-256d of pairs, the last node of an odd level paired with
/// itself; txids in internal byte order.
pub fn txid_merkle_root(txids: &[[u8; 32]]) -> Result<[u8; 32]> {
    ensure!(!txids.is_empty(), "a block has at least its coinbase transaction");
    let mut level = txids.to_vec();
    while level.len() > 1 {
        if level.len() % 2 == 1 {
            level.push(*level.last().expect("non-empty"));
        }
        level = level.chunks(2).map(|p| sha256d(&[p[0], p[1]].concat())).collect();
    }
    Ok(level[0])
}

/// The SHA-256d Merkle path (siblings, bottom-up) of the transaction at `index` among a block's `txids`, as
/// `seam_payment::txid_path_root` walks it.
pub fn txid_merkle_path(txids: &[[u8; 32]], index: usize) -> Result<Vec<[u8; 32]>> {
    ensure!(index < txids.len(), "the block has no transaction {index}");
    let mut level = txids.to_vec();
    let mut i = index;
    let mut path = vec![];
    while level.len() > 1 {
        if level.len() % 2 == 1 {
            level.push(*level.last().expect("non-empty"));
        }
        path.push(level[i ^ 1]);
        level = level.chunks(2).map(|p| sha256d(&[p[0], p[1]].concat())).collect();
        i /= 2;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_matches_the_compact_target() {
        // Testnet's PoWLimit (nBits 0x2007ffff) has work 32 (contracts/src/relay/README.md).
        assert_eq!(block_work(0x2007_ffff), U256::from(32u8));
        // 0x03000001 is target 1, so its work is 2^256 / 2.
        assert_eq!(block_work(0x0300_0001), U256::one() << 255);
        // A small exponent shifts the mantissa right: 0x01003456 is target 0, which has no work.
        assert_eq!(target_from_bits(0x0100_3456), Some(U256::zero()));
        assert_eq!(block_work(0x0100_3456), U256::zero());
        // Negative and overflowing targets have none either (SetCompact refuses them).
        assert_eq!(target_from_bits(0x0480_0001), None);
        assert_eq!(target_from_bits(0x2301_0000), None);
        assert_eq!(block_work(0xff7f_ffff), U256::zero());
    }

    #[test]
    fn the_auth_data_root_pads_to_a_power_of_two() {
        let a = [1u8; 32];
        assert_eq!(auth_data_root(&[a]).unwrap(), a, "one transaction: the root is its commitment");
        let three = auth_data_root(&[a, [2u8; 32], [3u8; 32]]).unwrap();
        let left = blake2b_256(b"ZcashAuthDatHash", &[&a, &[2u8; 32]]);
        let right = blake2b_256(b"ZcashAuthDatHash", &[&[3u8; 32], &[0u8; 32]]);
        assert_eq!(three, blake2b_256(b"ZcashAuthDatHash", &[&left, &right]));
        assert!(auth_data_root(&[]).is_err());
    }

    /// Every transaction's path leads to the root `seam_payment::txid_path_root` computes, for every block size up to
    /// 20 (odd levels repeat their last node).
    #[test]
    fn txid_paths_reach_the_merkle_root() {
        for n in 1..=20u8 {
            let txids: Vec<[u8; 32]> = (0..n).map(|i| [i + 1; 32]).collect();
            let root = txid_merkle_root(&txids).unwrap();
            for (i, txid) in txids.iter().enumerate() {
                let path = txid_merkle_path(&txids, i).unwrap();
                assert_eq!(seam_payment::txid_path_root(*txid, i as u32, &path), root, "tx {i} of {n}");
            }
            assert!(txid_merkle_path(&txids, n as usize).is_err());
        }
        assert_eq!(txid_merkle_root(&[[9; 32]]).unwrap(), [9; 32]);
    }

    #[test]
    fn display_hashes_are_reversed() {
        let mut h = [0u8; 32];
        h[0] = 0xab;
        let shown = display_hash(&h);
        assert!(shown.ends_with("ab") && shown.starts_with("00"));
        assert_eq!(parse_display_hash(&shown).unwrap(), h);
        assert!(parse_display_hash("00").is_err());
    }

    #[test]
    fn a_summary_serializes_its_hash_in_display_order() {
        let mut hash = [0u8; 32];
        hash[31] = 0x0a;
        let s = BlockSummary {
            height: 7,
            hash,
            time: 1,
            bits: 0x2007_ffff,
            sapling_root: [1; 32],
            orchard_root: [2; 32],
            ironwood_root: [3; 32],
            sapling_tx: 0,
            orchard_tx: 1,
            ironwood_tx: 2,
        };
        let json = serde_json::to_value(&s).unwrap();
        assert!(json["hash"].as_str().unwrap().starts_with("0a00"));
        assert_eq!(json["ironwood_root"], hex::encode([3u8; 32]));
        assert_eq!(serde_json::from_value::<BlockSummary>(json).unwrap(), s);
    }
}
