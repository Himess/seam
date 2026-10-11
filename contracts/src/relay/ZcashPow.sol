// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {SafeCast} from "@openzeppelin/contracts/utils/math/SafeCast.sol";

/// @title ZcashPow
/// @notice Zcash proof-of-work arithmetic: compact nBits encoding, block work, the difficulty adjustment formula,
///         the block hash.
/// @dev References (Zebra v7.0.0-rc.0, commit 6d1e414d6f55e4180d0e47baaa934bf97d5b4fec):
///        - zebra-chain/src/work/difficulty.rs: `CompactDifficulty::to_expanded`, `ExpandedDifficulty::to_compact`,
///          `Work::try_from` (zcashd `arith_uint256::SetCompact` / `GetCompact` / `GetBlockProof`);
///        - zebra-state/src/service/check/difficulty.rs: `AdjustedDifficulty::threshold_bits`,
///          `mean_target_difficulty`, `median_timespan_bounded`, `median_time`.
///      Zcash Protocol Specification § 7.7.3 "Difficulty adjustment", § 7.7.4 "nBits conversion", § 7.7.5
///      "Definition of Work", as amended by ZIP 218 (25 s spacing and a 102-block averaging window from NU7).
library ZcashPow {
    /// @notice PoWDampingFactor.
    int256 internal constant DAMPING_FACTOR = 4;
    /// @notice PoWMaxAdjustUp, in percent.
    int256 internal constant MAX_ADJUST_UP_PERCENT = 16;
    /// @notice PoWMaxAdjustDown, in percent.
    int256 internal constant MAX_ADJUST_DOWN_PERCENT = 32;

    /// @notice Decodes nBits into a target (zcashd `SetCompact`).
    /// @return target The expanded target.
    /// @return valid False for negative, zero or overflowing encodings, which consensus rejects
    ///         (Zebra `to_expanded` returns None for them).
    function decodeBits(uint256 bits) internal pure returns (uint256 target, bool valid) {
        uint256 size = (bits >> 24) & 0xff;
        uint256 word = bits & 0x007fffff;
        if (size <= 3) {
            target = word >> (8 * (3 - size));
        } else {
            if (word != 0 && (size > 34 || (word > 0xff && size > 33) || (word > 0xffff && size > 32))) {
                return (0, false);
            }
            target = word << (8 * (size - 3));
        }
        bool negative = word != 0 && (bits & 0x00800000) != 0;
        valid = !negative && target != 0;
    }

    /// @notice Encodes a target as nBits (zcashd `GetCompact`, equivalent to Zebra `to_compact`). Zero maps to 0.
    function encodeBits(uint256 target) internal pure returns (uint32) {
        if (target == 0) return 0;
        uint256 size = (_bitLength(target) + 7) / 8;
        uint256 compact = size <= 3 ? target << (8 * (3 - size)) : target >> (8 * (size - 3));
        if (compact & 0x00800000 != 0) {
            compact >>= 8;
            size += 1;
        }
        return SafeCast.toUint32(compact | (size << 24));
    }

    /// @notice Work of a block with this target: floor(2^256 / (target + 1)), computed as
    ///         ~target / (target + 1) + 1 (zcashd `GetBlockProof`, Zebra `Work::try_from`).
    /// @dev `target` must be below 2^256 - 1 (any valid target is).
    function work(uint256 target) internal pure returns (uint256) {
        return (~target / (target + 1)) + 1;
    }

    /// @notice ThresholdBits from the mean target and the two medians (Zebra `threshold_bits`).
    /// @param meanTarget floor(sum of the window's targets / window), see `meanTarget`.
    /// @param newerMedian MedianTime(height): median of the 11 times before the candidate.
    /// @param olderMedian MedianTime(height - window).
    /// @param averagingWindowTimespan AveragingWindowTimespan(height) = window * target spacing, in seconds.
    /// @param powLimit PoWLimit (already rounded through the compact form).
    function thresholdBits(
        uint256 meanTarget,
        uint256 newerMedian,
        uint256 olderMedian,
        uint256 averagingWindowTimespan,
        uint256 powLimit
    ) internal pure returns (uint32) {
        int256 awt = SafeCast.toInt256(averagingWindowTimespan);
        int256 actual = SafeCast.toInt256(newerMedian) - SafeCast.toInt256(olderMedian);
        // ActualTimespanDamped: Solidity's signed division truncates towards zero, as Zebra's
        // `Duration::num_seconds` does.
        int256 damped = awt + (actual - awt) / DAMPING_FACTOR;
        // MinActualTimespan / MaxActualTimespan. For the windows used (1275 s and 2550 s) these are whole seconds
        // (1071 / 1683 and 2142 / 3366), so integer division matches Zebra's `Duration` arithmetic exactly.
        int256 minTimespan = (awt * (100 - MAX_ADJUST_UP_PERCENT)) / 100;
        int256 maxTimespan = (awt * (100 + MAX_ADJUST_DOWN_PERCENT)) / 100;
        if (damped < minTimespan) damped = minTimespan;
        if (damped > maxTimespan) damped = maxTimespan;
        // Divide first, then multiply, exactly like zcashd and Zebra: consensus depends on this rounding, and the
        // other order gives different nBits on real Testnet blocks (checked against Testnet data, see README).
        // forge-lint: disable-next-line(divide-before-multiply)
        uint256 threshold = (meanTarget / averagingWindowTimespan) * SafeCast.toUint256(damped);
        if (threshold > powLimit) threshold = powLimit;
        return encodeBits(threshold);
    }

    /// @notice Median of the given times (sorted(S)[len / 2], Zebra `median_time`). Sorts `times` in place
    ///         (insertion sort: close to linear when the times are mostly increasing). `times` must not be empty.
    function median(uint256[] memory times) internal pure returns (uint256 m) {
        assembly ("memory-safe") {
            let n := mload(times)
            let p := add(times, 32)
            let end := add(p, shl(5, n))
            for { let i := add(p, 32) } lt(i, end) { i := add(i, 32) } {
                let v := mload(i)
                let j := i
                for {} gt(j, p) { j := sub(j, 32) } {
                    let prev := mload(sub(j, 32))
                    if iszero(gt(prev, v)) { break }
                    mstore(j, prev)
                }
                mstore(j, v)
            }
            m := mload(add(p, shl(5, shr(1, n))))
        }
    }

    /// @notice Block hash: SHA256d of the serialized header, in internal byte order (as in hashPrevBlock).
    ///         Block explorers display it byte-reversed.
    function blockHash(bytes memory header) internal pure returns (bytes32) {
        return sha256(abi.encodePacked(sha256(header)));
    }

    /// @notice The hash read as the little-endian 256-bit integer that is compared with the target.
    function hashToUint(bytes32 hash) internal pure returns (uint256) {
        return reverseBytes32(uint256(hash));
    }

    /// @notice Reverses the byte order of a 32-byte word.
    function reverseBytes32(uint256 v) internal pure returns (uint256) {
        v = ((v & 0xff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00) >> 8)
            | ((v & 0x00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff) << 8);
        v = ((v & 0xffff0000ffff0000ffff0000ffff0000ffff0000ffff0000ffff0000ffff0000) >> 16)
            | ((v & 0x0000ffff0000ffff0000ffff0000ffff0000ffff0000ffff0000ffff0000ffff) << 16);
        v = ((v & 0xffffffff00000000ffffffff00000000ffffffff00000000ffffffff00000000) >> 32)
            | ((v & 0x00000000ffffffff00000000ffffffff00000000ffffffff00000000ffffffff) << 32);
        v = ((v & 0xffffffffffffffff0000000000000000ffffffffffffffff0000000000000000) >> 64)
            | ((v & 0x0000000000000000ffffffffffffffff0000000000000000ffffffffffffffff) << 64);
        return (v >> 128) | (v << 128);
    }

    /// @dev Number of significant bits (0 for 0).
    function _bitLength(uint256 x) private pure returns (uint256 n) {
        if (x >> 128 != 0) {
            x >>= 128;
            n += 128;
        }
        if (x >> 64 != 0) {
            x >>= 64;
            n += 64;
        }
        if (x >> 32 != 0) {
            x >>= 32;
            n += 32;
        }
        if (x >> 16 != 0) {
            x >>= 16;
            n += 16;
        }
        if (x >> 8 != 0) {
            x >>= 8;
            n += 8;
        }
        if (x >> 4 != 0) {
            x >>= 4;
            n += 4;
        }
        if (x >> 2 != 0) {
            x >>= 2;
            n += 2;
        }
        if (x >> 1 != 0) {
            x >>= 1;
            n += 1;
        }
        if (x != 0) n += 1;
    }
}
