// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {SafeCast} from "@openzeppelin/contracts/utils/math/SafeCast.sol";
import {IZcashHeaderRelay} from "./IZcashHeaderRelay.sol";
import {ZcashEquihash} from "./ZcashEquihash.sol";
import {ZcashPow} from "./ZcashPow.sol";

/// @title ZcashHeaderRelay
/// @notice Zcash Testnet block-header relay. Starting from a trusted checkpoint it accepts headers that pass the
///         header consensus rules and keeps the best chain by cumulative work.
/// @dev Tier 2 foundation (docs/TIER2.md): not wired into the Registry. Each submitted header must
///        1. be a 1487-byte v4 header (version >= 4 as an int32) with a 1344-byte solution, whose parent is known;
///        2. have time > median-time-past of the 11 previous blocks, time <= that median + 90 min (Testnet, from
///           height 653,606) and time <= block.timestamp + 2 h (Zebra's local-clock rule, Base's clock here);
///        3. carry nBits equal to the difficulty adjustment (Testnet minimum-difficulty rule included);
///        4. carry a valid Equihash (200, 9) solution;
///        5. hash (SHA256d, little-endian) to at most the target decoded from nBits.
///      Rules and parameters follow Zebra v7.0.0-rc.0 (commit 6d1e414d6f55e4180d0e47baaa934bf97d5b4fec):
///      zebra-state/src/service/check.rs `difficulty_threshold_and_time_are_valid`,
///      zebra-state/src/service/check/difficulty.rs, zebra-consensus/src/block/check.rs, and ZIP 218 / ZIP 259:
///      from NU7 (Testnet height 4,465,026) the target spacing is 25 s and the averaging window 102 blocks (75 s and
///      17 before); the Testnet minimum-difficulty gap is 450 s on both sides (6 x 75 s, then 18 x 25 s).
///
///      Not verified here: anything inside the block (transactions, merkle root, block commitments). Those fields
///      are stored as committed by proof of work, for later inclusion proofs.
///
///      Storage. Every known block has a record (4 slots). For the best chain, block hashes are indexed by height,
///      and times and nBits are packed eight per slot by height, so the 102 + 11 previous blocks a difficulty
///      adjustment needs cost about 17 cold storage reads. A header on a side branch walks back to the best chain
///      first; the walk is as long as the branch.
contract ZcashHeaderRelay is IZcashHeaderRelay {
    // --- Zcash Testnet parameters ---------------------------------------------------------------------------

    /// @notice NU7 activation height on Testnet (ZIP 259).
    uint256 public constant NU7_ACTIVATION_HEIGHT = 4_465_026;
    /// @notice Lowest supported checkpoint height: from Testnet height 653,606 the rules implemented here (75 s
    ///         spacing since Blossom at 584,000, the 450 s minimum-difficulty gap since 299,188, the 90-minute
    ///         maximum time since 653,606) apply unchanged until NU7.
    uint256 public constant MIN_CHECKPOINT_HEIGHT = 653_606;
    /// @notice ToCompact(PoWLimit) for Testnet, PoWLimit = 2^251 - 1.
    uint32 public constant POW_LIMIT_BITS = 0x2007ffff;
    /// @notice PoWLimit rounded through its compact form, as Zebra uses it (0x07ffff << 232).
    uint256 public constant POW_LIMIT = 0x07ffff << 232;
    /// @notice PoWAveragingWindow(height) + PoWMedianBlockSpan for the largest window (102 + 11).
    uint256 public constant CONTEXT_LENGTH = 113;

    uint256 private constant HEADER_LENGTH = 1487;
    uint256 private constant MEDIAN_BLOCK_SPAN = 11;
    uint256 private constant PRE_NU7_AVERAGING_WINDOW = 17;
    uint256 private constant POST_NU7_AVERAGING_WINDOW = 102;
    uint256 private constant PRE_NU7_TARGET_SPACING = 75;
    uint256 private constant POST_NU7_TARGET_SPACING = 25;
    uint256 private constant PRE_NU7_MIN_DIFFICULTY_GAP_MULTIPLIER = 6;
    uint256 private constant POST_NU7_MIN_DIFFICULTY_GAP_MULTIPLIER = 18;
    uint256 private constant BLOCK_MAX_TIME_SINCE_MEDIAN = 90 minutes;
    uint256 private constant MAX_FUTURE_BLOCK_TIME = 2 hours;

    // --- State --------------------------------------------------------------------------------------------------

    struct BlockRecord {
        bytes32 prevHash;
        uint128 chainWork;
        uint32 height;
        uint32 time;
        uint32 bits;
        bytes32 merkleRoot;
        bytes32 blockCommitments;
    }

    /// @dev The blocks between a candidate's parent and the best chain (a side branch), parent first.
    struct Branch {
        uint256 forkHeight; // height of the candidate's last ancestor on the best chain
        bytes32[] hashes; // hashes[k], times[k], bits[k]: the block at height parentHeight - k
        uint256[] times;
        uint256[] bits;
    }

    /// @notice Trusted checkpoint (constructor input).
    bytes32 public immutable checkpointHash;
    uint256 public immutable checkpointHeight;

    mapping(bytes32 blockHash => BlockRecord) private _blocks;
    mapping(uint256 height => bytes32) private _bestHashAt;
    mapping(uint256 slot => uint256) private _bestTimes; // slot = height >> 3: eight uint32 times
    mapping(uint256 slot => uint256) private _bestBits; // slot = height >> 3: eight uint32 nBits
    bytes32 private _bestTip;

    /// @param checkpointHash_ Hash of the trusted checkpoint block (internal byte order).
    /// @param checkpointHeight_ Its height.
    /// @param contextTimes Times of the 113 blocks ending with the checkpoint, oldest first
    ///        (heights checkpointHeight_ - 112 .. checkpointHeight_).
    /// @param contextBits Their nBits, same order.
    constructor(
        bytes32 checkpointHash_,
        uint256 checkpointHeight_,
        uint32[] memory contextTimes,
        uint32[] memory contextBits
    ) {
        if (checkpointHash_ == bytes32(0) || checkpointHeight_ < MIN_CHECKPOINT_HEIGHT || checkpointHeight_ >= 2 ** 31)
        {
            revert InvalidCheckpoint();
        }
        if (contextTimes.length != CONTEXT_LENGTH || contextBits.length != CONTEXT_LENGTH) {
            revert InvalidContextLength();
        }
        uint256 first = checkpointHeight_ + 1 - CONTEXT_LENGTH;
        for (uint256 k; k < CONTEXT_LENGTH; ++k) {
            (uint256 target, bool valid) = ZcashPow.decodeBits(contextBits[k]);
            if (!valid || target > POW_LIMIT) revert InvalidContextBits(k);
            _setPacked(_bestTimes, first + k, contextTimes[k]);
            _setPacked(_bestBits, first + k, contextBits[k]);
        }
        _blocks[checkpointHash_] = BlockRecord({
            prevHash: bytes32(0),
            chainWork: 0,
            height: SafeCast.toUint32(checkpointHeight_),
            time: contextTimes[CONTEXT_LENGTH - 1],
            bits: contextBits[CONTEXT_LENGTH - 1],
            merkleRoot: bytes32(0),
            blockCommitments: bytes32(0)
        });
        _bestHashAt[checkpointHeight_] = checkpointHash_;
        _bestTip = checkpointHash_;
        checkpointHash = checkpointHash_;
        checkpointHeight = checkpointHeight_;
        emit BestChainUpdated(checkpointHash_, checkpointHeight_, bytes32(0), checkpointHeight_);
    }

    // --- Submission ---------------------------------------------------------------------------------------------

    /// @inheritdoc IZcashHeaderRelay
    function submitHeaders(bytes[] calldata headers) external {
        for (uint256 i; i < headers.length; ++i) {
            submitHeader(headers[i]);
        }
    }

    /// @inheritdoc IZcashHeaderRelay
    function submitHeader(bytes calldata header) public returns (bytes32 blockHash) {
        if (header.length != HEADER_LENGTH) revert InvalidHeaderLength(header.length);
        if (header[140] != 0xfd || header[141] != 0x40 || header[142] != 0x05) revert InvalidSolutionLength();
        bytes memory raw = header;
        blockHash = ZcashPow.blockHash(raw);
        if (_blocks[blockHash].height != 0) revert BlockAlreadyKnown(blockHash);

        int256 version = int256(int32(uint32(_readUint32(header, 0))));
        if (version < 4) revert InvalidVersion(version);
        bytes32 prevHash = bytes32(header[4:36]);
        BlockRecord storage parent = _blocks[prevHash];
        uint256 height = parent.height;
        if (height == 0) revert UnknownParent(prevHash);
        height += 1;
        uint256 time = _readUint32(header, 100);
        uint256 bits = _readUint32(header, 104);

        Branch memory branch = _branchOf(prevHash, height - 1);
        _checkTimeAndBits(branch, height, time, bits, parent.time);

        ZcashEquihash.Result equihash = ZcashEquihash.verify(raw);
        if (equihash != ZcashEquihash.Result.Valid) revert InvalidEquihashSolution(equihash);

        (uint256 target,) = ZcashPow.decodeBits(bits); // valid: bits equals a computed expected value
        if (ZcashPow.hashToUint(blockHash) > target) revert HashAboveTarget(blockHash, target);

        uint256 chainWork = uint256(parent.chainWork) + ZcashPow.work(target);
        _blocks[blockHash] = BlockRecord({
            prevHash: prevHash,
            chainWork: SafeCast.toUint128(chainWork),
            height: SafeCast.toUint32(height),
            time: SafeCast.toUint32(time),
            bits: SafeCast.toUint32(bits),
            merkleRoot: bytes32(header[36:68]),
            blockCommitments: bytes32(header[68:100])
        });
        emit HeaderAdded(blockHash, prevHash, height, chainWork);
        _updateBestChain(blockHash, height, chainWork, time, bits, branch);
    }

    // --- Views --------------------------------------------------------------------------------------------------

    /// @inheritdoc IZcashHeaderRelay
    function isInBestChain(bytes32 blockHash) public view returns (bool) {
        uint256 height = _blocks[blockHash].height;
        return height != 0 && _bestHashAt[height] == blockHash;
    }

    /// @inheritdoc IZcashHeaderRelay
    function confirmations(bytes32 blockHash) external view returns (uint256) {
        if (!isInBestChain(blockHash)) return 0;
        return uint256(_blocks[_bestTip].height) - _blocks[blockHash].height + 1;
    }

    /// @inheritdoc IZcashHeaderRelay
    function confirmationWork(bytes32 blockHash) external view returns (uint256) {
        if (!isInBestChain(blockHash)) return 0;
        return uint256(_blocks[_bestTip].chainWork) - _blocks[blockHash].chainWork;
    }

    /// @inheritdoc IZcashHeaderRelay
    function headerOf(bytes32 blockHash) external view returns (HeaderInfo memory info) {
        BlockRecord storage b = _blocks[blockHash];
        if (b.height == 0) return info;
        info = HeaderInfo({
            known: true,
            prevHash: b.prevHash,
            height: b.height,
            time: b.time,
            bits: b.bits,
            chainWork: b.chainWork,
            merkleRoot: b.merkleRoot,
            blockCommitments: b.blockCommitments
        });
    }

    /// @inheritdoc IZcashHeaderRelay
    function bestChainTip() external view returns (bytes32 blockHash, uint256 height, uint256 chainWork) {
        blockHash = _bestTip;
        BlockRecord storage b = _blocks[blockHash];
        return (blockHash, b.height, b.chainWork);
    }

    /// @inheritdoc IZcashHeaderRelay
    function bestBlockHashAt(uint256 height) external view returns (bytes32) {
        return _bestHashAt[height];
    }

    // --- Contextual checks --------------------------------------------------------------------------------------

    /// @dev Header time rules and the nBits rule (Zebra `difficulty_threshold_and_time_are_valid`).
    function _checkTimeAndBits(Branch memory branch, uint256 height, uint256 time, uint256 bits, uint256 parentTime)
        private
        view
    {
        // MedianTime(height): median of the times of the 11 previous blocks.
        uint256 medianTimePast = _medianTime(branch, height - 1);
        if (time <= medianTimePast) revert TimeTooEarly(time, medianTimePast);
        if (time > medianTimePast + BLOCK_MAX_TIME_SINCE_MEDIAN) {
            revert TimeTooLate(time, medianTimePast + BLOCK_MAX_TIME_SINCE_MEDIAN);
        }
        if (time > block.timestamp + MAX_FUTURE_BLOCK_TIME) {
            revert TimeTooFarInFuture(time, block.timestamp + MAX_FUTURE_BLOCK_TIME);
        }
        uint256 expected = _expectedBits(branch, height, time, parentTime, medianTimePast);
        if (bits != expected) revert BadDifficultyBits(bits, expected);
    }

    /// @dev ThresholdBits(height), or ToCompact(PoWLimit) for a Testnet minimum-difficulty block
    ///      (Zebra `AdjustedDifficulty::expected_difficulty_threshold`).
    function _expectedBits(Branch memory branch, uint256 height, uint256 time, uint256 parentTime, uint256 newerMedian)
        private
        view
        returns (uint256)
    {
        bool nu7 = height >= NU7_ACTIVATION_HEIGHT;
        uint256 spacing = nu7 ? POST_NU7_TARGET_SPACING : PRE_NU7_TARGET_SPACING;

        // Testnet minimum-difficulty rule (ZIP 205 / 208, ZIP 218 § "Minimum difficulty blocks on Testnet"): a block
        // whose time is MORE than the gap after its parent's time must carry ToCompact(PoWLimit). The gap is
        // 6 x 75 s before NU7 and 18 x 25 s from NU7: 450 s either way.
        uint256 gap = spacing * (nu7 ? POST_NU7_MIN_DIFFICULTY_GAP_MULTIPLIER : PRE_NU7_MIN_DIFFICULTY_GAP_MULTIPLIER);
        if (time > parentTime + gap) return POW_LIMIT_BITS;

        uint256 window = nu7 ? POST_NU7_AVERAGING_WINDOW : PRE_NU7_AVERAGING_WINDOW;
        // MedianTime(height - window): median of the 11 times before the window.
        uint256 olderMedian = _medianTime(branch, height - 1 - window);
        return ZcashPow.thresholdBits(
            _meanTarget(branch, height - 1, window), newerMedian, olderMedian, window * spacing, POW_LIMIT
        );
    }

    /// @dev Median of the times of the 11 blocks at heights top - 10 .. top on the candidate's chain. The times are
    ///      loaded oldest first, so the insertion sort in `ZcashPow.median` is close to linear on real chains.
    function _medianTime(Branch memory branch, uint256 top) private view returns (uint256) {
        uint256 from = top + 1 - MEDIAN_BLOCK_SPAN;
        uint256[] memory times = new uint256[](MEDIAN_BLOCK_SPAN);
        uint256 fork = branch.forkHeight;
        if (from <= fork) _readPacked(_slotOf(_bestTimes), from, top < fork ? top : fork, times);
        if (top > fork) {
            // heights above the fork: branch.times[k] is the block at height fork + length - k
            uint256 branchTop = fork + branch.times.length;
            for (uint256 h = from > fork ? from : fork + 1; h <= top; ++h) {
                times[h - from] = branch.times[branchTop - h];
            }
        }
        return ZcashPow.median(times);
    }

    /// @dev MeanTarget: floor(sum of the targets of the `window` blocks up to `parentHeight` / window). Zebra sums
    ///      quotients and remainders separately because 102 Testnet targets can exceed 2^256; the result is exactly
    ///      the floor of the mean.
    function _meanTarget(Branch memory branch, uint256 parentHeight, uint256 window) private view returns (uint256) {
        uint256 lowest = parentHeight + 1 - window;
        uint256 fork = branch.forkHeight;
        uint256 quotients;
        uint256 remainders;
        // Side-branch blocks (heights above the fork), parent first.
        for (uint256 k; k < branch.bits.length && parentHeight - k >= lowest; ++k) {
            uint256 target = _target(branch.bits[k]);
            quotients += target / window;
            remainders += target % window;
        }
        // Best-chain blocks, lowest .. fork, straight from the packed nBits.
        if (fork >= lowest) {
            uint256 slotBase = _slotOf(_bestBits);
            assembly ("memory-safe") {
                let cachedKey := not(0)
                let cached := 0
                for { let h := lowest } iszero(gt(h, fork)) { h := add(h, 1) } {
                    let key := shr(3, h)
                    if iszero(eq(key, cachedKey)) {
                        cachedKey := key
                        mstore(0x00, key)
                        mstore(0x20, slotBase)
                        cached := sload(keccak256(0x00, 0x40))
                    }
                    let b := and(shr(shl(5, and(h, 7)), cached), 0xffffffff)
                    // Stored nBits were validated on entry (positive, no overflow): plain decoding is exact.
                    let size := shr(24, b)
                    let word := and(b, 0x7fffff)
                    let t := 0
                    switch lt(size, 4)
                    case 1 { t := shr(shl(3, sub(3, size)), word) }
                    default { t := shl(shl(3, sub(size, 3)), word) }
                    quotients := add(quotients, div(t, window))
                    remainders := add(remainders, mod(t, window))
                }
            }
        }
        return quotients + remainders / window;
    }

    /// @dev Target of nBits that were validated when they were stored.
    function _target(uint256 bits) private pure returns (uint256 target) {
        (target,) = ZcashPow.decodeBits(bits);
    }

    /// @dev Writes the packed values of heights from .. to (inclusive) into out[0 ..].
    function _readPacked(uint256 slotBase, uint256 from, uint256 to, uint256[] memory out) private view {
        assembly ("memory-safe") {
            let cachedKey := not(0)
            let cached := 0
            let dst := add(out, 32)
            for { let h := from } iszero(gt(h, to)) { h := add(h, 1) } {
                let key := shr(3, h)
                if iszero(eq(key, cachedKey)) {
                    cachedKey := key
                    mstore(0x00, key)
                    mstore(0x20, slotBase)
                    cached := sload(keccak256(0x00, 0x40))
                }
                mstore(dst, and(shr(shl(5, and(h, 7)), cached), 0xffffffff))
                dst := add(dst, 32)
            }
        }
    }

    /// @dev Storage slot of a packed best-chain mapping (its entries are at keccak256(key . slot)).
    function _slotOf(mapping(uint256 => uint256) storage packed) private pure returns (uint256 slot) {
        assembly ("memory-safe") {
            slot := packed.slot
        }
    }

    // --- Chain bookkeeping --------------------------------------------------------------------------------------

    /// @dev The side branch from `parentHash` down to the best chain (empty when the parent is on the best chain).
    ///      Every stored block descends from the checkpoint, which is always on the best chain, so the walk ends.
    function _branchOf(bytes32 parentHash, uint256 parentHeight) private view returns (Branch memory branch) {
        bytes32 h = parentHash;
        uint256 height = parentHeight;
        while (_bestHashAt[height] != h) {
            h = _blocks[h].prevHash;
            --height;
        }
        uint256 depth = parentHeight - height;
        branch.forkHeight = height;
        branch.hashes = new bytes32[](depth);
        branch.times = new uint256[](depth);
        branch.bits = new uint256[](depth);
        h = parentHash;
        for (uint256 k; k < depth; ++k) {
            BlockRecord storage b = _blocks[h];
            branch.hashes[k] = h;
            branch.times[k] = b.time;
            branch.bits[k] = b.bits;
            h = b.prevHash;
        }
    }

    /// @dev Makes the new block the tip if it has strictly more work than the current tip (ties keep the block seen
    ///      first). On a reorganisation the side branch is written into the best-chain indexes and the heights above
    ///      the new tip are cleared.
    function _updateBestChain(
        bytes32 blockHash,
        uint256 height,
        uint256 chainWork,
        uint256 time,
        uint256 bits,
        Branch memory branch
    ) private {
        bytes32 previousTip = _bestTip;
        BlockRecord storage tip = _blocks[previousTip];
        if (chainWork <= tip.chainWork) return;
        uint256 previousHeight = tip.height;

        uint256 depth = branch.hashes.length;
        for (uint256 k; k < depth; ++k) {
            uint256 h = height - 1 - k;
            _bestHashAt[h] = branch.hashes[k];
            _setPacked(_bestTimes, h, branch.times[k]);
            _setPacked(_bestBits, h, branch.bits[k]);
        }
        _bestHashAt[height] = blockHash;
        _setPacked(_bestTimes, height, time);
        _setPacked(_bestBits, height, bits);
        for (uint256 h = height + 1; h <= previousHeight; ++h) {
            delete _bestHashAt[h];
        }
        _bestTip = blockHash;
        emit BestChainUpdated(blockHash, height, previousTip, branch.forkHeight);
    }

    function _setPacked(mapping(uint256 => uint256) storage packed, uint256 height, uint256 value) private {
        uint256 slot = height >> 3;
        uint256 shift = (height & 7) << 5;
        packed[slot] = (packed[slot] & ~(uint256(0xffffffff) << shift)) | (value << shift);
    }

    /// @dev Little-endian uint32 at `offset`.
    function _readUint32(bytes calldata data, uint256 offset) private pure returns (uint256 v) {
        assembly ("memory-safe") {
            let w := calldataload(add(data.offset, offset))
            v := or(or(byte(0, w), shl(8, byte(1, w))), or(shl(16, byte(2, w)), shl(24, byte(3, w))))
        }
    }
}
