//! Membership paths in the ZIP 221 history tree. Ported from Zecured's `signer/crates/deposit-witness/src/mmr.rs`
//! (73de9f909e270e8aea10c2b221905f62a6a4c4df).
//!
//! The tree is a Merkle mountain range: its leaves split into perfect binary trees (peaks), largest and oldest first,
//! one per set bit of the leaf count, and the root is the left fold of the peaks with the node-combining function (as
//! `zcash_history::Tree` builds it). A path is a leaf's siblings up to its peak; with all the peaks it gives the root,
//! which is what `seam_payment::check_inclusion` checks.

use anyhow::{Result, ensure};
use zcash_history::{NodeDataV3, V3, Version};

/// One step of a path: the sibling node and whether it is the left child.
pub type PathStep = (NodeDataV3, bool);

/// A history tree over its leaves, oldest first.
pub struct Mmr {
    leaves: Vec<NodeDataV3>,
}

impl Mmr {
    pub fn new(leaves: Vec<NodeDataV3>) -> Result<Self> {
        ensure!(!leaves.is_empty(), "an empty history");
        Ok(Self { leaves })
    }

    pub fn len(&self) -> usize {
        self.leaves.len()
    }

    pub fn is_empty(&self) -> bool {
        self.leaves.is_empty()
    }

    pub fn leaf(&self, index: usize) -> Option<&NodeDataV3> {
        self.leaves.get(index)
    }

    /// The perfect subtrees, oldest first, as (first leaf, size).
    fn peak_ranges(&self) -> Vec<(usize, usize)> {
        peak_ranges(self.leaves.len())
    }

    fn subtree(&self, start: usize, size: usize) -> NodeDataV3 {
        if size == 1 {
            return self.leaves[start].clone();
        }
        let half = size / 2;
        V3::combine(&self.subtree(start, half), &self.subtree(start + half, half))
    }

    /// The peaks, oldest first.
    pub fn peaks(&self) -> Vec<NodeDataV3> {
        self.peak_ranges().into_iter().map(|(s, n)| self.subtree(s, n)).collect()
    }

    /// The root node: the left fold of the peaks.
    pub fn root(&self) -> NodeDataV3 {
        bag_peaks(&self.peaks()).expect("a history has a peak")
    }

    /// The path of leaf `index` up to its peak (bottom-up), and which peak that is.
    pub fn path(&self, index: usize) -> Result<(Vec<PathStep>, usize)> {
        ensure!(index < self.leaves.len(), "the history has no leaf {index}");
        let (peak_index, (mut start, mut size)) = self
            .peak_ranges()
            .into_iter()
            .enumerate()
            .find(|(_, (s, n))| index >= *s && index < s + n)
            .expect("every leaf is under a peak");
        let mut top_down = vec![];
        while size > 1 {
            let half = size / 2;
            if index < start + half {
                top_down.push((self.subtree(start + half, half), false));
            } else {
                top_down.push((self.subtree(start, half), true));
                start += half;
            }
            size = half;
        }
        top_down.reverse();
        Ok((top_down, peak_index))
    }
}

/// The perfect subtrees of a history of `leaves` leaves, oldest first, as (first leaf, size).
pub fn peak_ranges(leaves: usize) -> Vec<(usize, usize)> {
    let mut out = vec![];
    let mut start = 0;
    for bit in (0..usize::BITS).rev() {
        let size = 1usize << bit;
        if leaves & size != 0 {
            out.push((start, size));
            start += size;
        }
    }
    out
}

/// The node a path leads to from `leaf`: its peak.
pub fn fold_path(leaf: &NodeDataV3, path: &[PathStep]) -> NodeDataV3 {
    path.iter().fold(leaf.clone(), |acc, (sibling, sibling_is_left)| {
        if *sibling_is_left {
            V3::combine(sibling, &acc)
        } else {
            V3::combine(&acc, sibling)
        }
    })
}

/// The root of a history whose peaks are `peaks` (oldest first): their left fold.
pub fn bag_peaks(peaks: &[NodeDataV3]) -> Option<NodeDataV3> {
    let (first, rest) = peaks.split_first()?;
    Some(rest.iter().fold(first.clone(), |acc, p| V3::combine(&acc, p)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{BlockSummary, history_root, leaf};

    pub(crate) fn leaves(n: u32) -> Vec<NodeDataV3> {
        (0..n)
            .map(|i| {
                leaf(
                    seam_payment::NU7_BRANCH_ID,
                    &BlockSummary {
                        height: 100 + i,
                        hash: [i as u8; 32],
                        time: 1_000 + i,
                        bits: 0x1d00_ffff,
                        sapling_root: [1; 32],
                        orchard_root: [2; 32],
                        ironwood_root: [3; 32],
                        sapling_tx: u64::from(i % 2),
                        orchard_tx: 0,
                        ironwood_tx: 1,
                    },
                )
            })
            .collect()
    }

    /// Our peaks and fold give the root zcash_history's incremental tree gives, for every size up to 40, and every
    /// path leads to its peak.
    #[test]
    fn roots_and_paths_match_zcash_history() {
        for n in 1..=40 {
            let ls = leaves(n);
            let mmr = Mmr::new(ls.clone()).unwrap();
            assert_eq!(V3::hash(&mmr.root()), history_root(&ls).unwrap(), "root of {n} leaves");
            let peaks = mmr.peaks();
            assert_eq!(peaks.len(), n.count_ones() as usize);
            for (i, l) in ls.iter().enumerate() {
                let (path, peak) = mmr.path(i).unwrap();
                assert_eq!(V3::to_bytes(&fold_path(l, &path)), V3::to_bytes(&peaks[peak]), "path of leaf {i} of {n}");
            }
            assert!(mmr.path(n as usize).is_err());
        }
        assert!(Mmr::new(vec![]).is_err());
        assert!(bag_peaks(&[]).is_none());
    }

    #[test]
    fn peaks_follow_the_bits_of_the_leaf_count() {
        assert_eq!(peak_ranges(1), [(0, 1)]);
        assert_eq!(peak_ranges(6), [(0, 4), (4, 2)]);
        assert_eq!(peak_ranges(7), [(0, 4), (4, 2), (6, 1)]);
        assert_eq!(peak_ranges(0), []);
    }
}
