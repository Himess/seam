// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {console} from "forge-std/console.sol";
import {RelayFixtures} from "./RelayFixtures.sol";
import {ZcashHeaderRelay} from "../../src/relay/ZcashHeaderRelay.sol";
import {IZcashHeaderRelay} from "../../src/relay/IZcashHeaderRelay.sol";
import {ZcashEquihash} from "../../src/relay/ZcashEquihash.sol";
import {ZcashPow} from "../../src/relay/ZcashPow.sol";

/// @notice ZcashHeaderRelay against real Zcash Testnet headers and synthetic forks (see src/relay/README.md).
contract ZcashHeaderRelayTest is RelayFixtures {
    uint256 internal constant MIN_DIFFICULTY_WORK = 32; // work of nBits 0x2007ffff

    // --- real headers -------------------------------------------------------------------------------------------

    function test_constructor_checkpointIsTheTip() public {
        RealFixture memory fx = _loadReal(RECENT);
        ZcashHeaderRelay relay = _deploy(fx);
        (bytes32 tip, uint256 height, uint256 work) = relay.bestChainTip();
        assertEq(tip, fx.checkpointHash);
        assertEq(height, fx.checkpointHeight);
        assertEq(work, 0);
        assertTrue(relay.isInBestChain(fx.checkpointHash));
        assertEq(relay.confirmations(fx.checkpointHash), 1);
        assertEq(relay.bestBlockHashAt(fx.checkpointHeight), fx.checkpointHash);
        IZcashHeaderRelay.HeaderInfo memory info = relay.headerOf(fx.checkpointHash);
        assertTrue(info.known);
        assertEq(info.time, fx.contextTimes[112]);
        assertEq(info.bits, fx.contextBits[112]);
    }

    /// @notice 32 consecutive post-NU7 headers (25 s spacing, 102-block window), each verified in full.
    function test_realHeaders_recentPostNu7Run() public {
        _submitAndCheckRun(_loadReal(RECENT));
    }

    /// @notice 30 headers across the NU7 activation: 11 under the pre-NU7 rules (75 s, 17-block window), then 19
    ///         under ZIP 218 (25 s, 102-block window), including the activation block 4,465,026.
    function test_realHeaders_acrossNu7Activation() public {
        RealFixture memory fx = _loadReal(NU7_ACTIVATION);
        assertEq(fx.heights[11], 4_465_026);
        _submitAndCheckRun(fx);
        // The activation block's nBits follow the 102-block window (a 17-block window would give 0x1f00959e).
        assertEq(_bits(fx.headers[11]), 0x1f0162f6);
    }

    function _submitAndCheckRun(RealFixture memory fx) internal {
        ZcashHeaderRelay relay = _deploy(fx);
        uint256 n = fx.headers.length;
        for (uint256 i; i < n; ++i) {
            _warpTo(fx.headers[i]);
            vm.expectEmit(true, true, false, false, address(relay));
            emit IZcashHeaderRelay.HeaderAdded(fx.hashes[i], i == 0 ? fx.checkpointHash : fx.hashes[i - 1], 0, 0);
            bytes32 h = relay.submitHeader(fx.headers[i]);
            assertEq(h, fx.hashes[i], "hash");
            (bytes32 tip, uint256 height,) = relay.bestChainTip();
            assertEq(tip, h, "tip");
            assertEq(height, fx.heights[i], "height");
        }
        assertEq(relay.confirmations(fx.checkpointHash), n + 1);
        assertEq(relay.confirmations(fx.hashes[0]), n);
        assertEq(relay.confirmations(fx.hashes[n - 1]), 1);
        for (uint256 i; i < n; ++i) {
            assertTrue(relay.isInBestChain(fx.hashes[i]));
            assertEq(relay.bestBlockHashAt(fx.heights[i]), fx.hashes[i]);
        }
        // Work is strictly increasing along the chain, and confirmationWork is what was added on top.
        (,, uint256 tipWork) = relay.bestChainTip();
        assertEq(relay.confirmationWork(fx.hashes[0]), tipWork - relay.headerOf(fx.hashes[0]).chainWork);
        assertEq(relay.confirmationWork(fx.hashes[n - 1]), 0);
    }

    function test_realHeaders_storedFields() public {
        RealFixture memory fx = _loadReal(RECENT);
        ZcashHeaderRelay relay = _deploy(fx);
        _warpTo(fx.headers[0]);
        relay.submitHeader(fx.headers[0]);
        bytes memory h = fx.headers[0];
        IZcashHeaderRelay.HeaderInfo memory info = relay.headerOf(fx.hashes[0]);
        assertTrue(info.known);
        assertEq(info.prevHash, fx.checkpointHash);
        assertEq(info.height, fx.heights[0]);
        assertEq(info.time, _time(h));
        assertEq(info.bits, _bits(h));
        assertEq(info.merkleRoot, _bytes32At(h, 36));
        assertEq(info.blockCommitments, _bytes32At(h, 68));
        (uint256 target,) = ZcashPow.decodeBits(_bits(h));
        assertEq(info.chainWork, ZcashPow.work(target));
        assertFalse(relay.headerOf(keccak256("unknown")).known);
    }

    function test_submitHeaders_batch() public {
        RealFixture memory fx = _loadReal(RECENT);
        ZcashHeaderRelay relay = _deploy(fx);
        vm.warp(_time(fx.headers[fx.headers.length - 1]) + 60);
        relay.submitHeaders(fx.headers);
        (bytes32 tip, uint256 height,) = relay.bestChainTip();
        assertEq(tip, fx.hashes[fx.hashes.length - 1]);
        assertEq(height, fx.heights[fx.heights.length - 1]);
    }

    /// @notice Gas per header that extends the best chain, over the 32 real post-NU7 headers.
    /// @dev Run with `--isolate` for on-chain numbers: every submission is then its own transaction (cold storage)
    ///      and the measured value is the whole transaction (21,000 + calldata + execution). Without `--isolate` all
    ///      32 calls share one transaction, so storage is partly warm and only execution is measured.
    function test_gas_submitOneHeader() public {
        RealFixture memory fx = _loadReal(RECENT);
        ZcashHeaderRelay relay = _deploy(fx);
        uint256 measured;
        uint256 sum;
        uint256 maxGas;
        uint256 minGas = type(uint256).max;
        for (uint256 i; i < fx.headers.length; ++i) {
            _warpTo(fx.headers[i]);
            uint256 g = gasleft();
            relay.submitHeader(fx.headers[i]);
            measured = g - gasleft();
            sum += measured;
            if (measured > maxGas) maxGas = measured;
            if (measured < minGas) minGas = measured;
        }
        bytes memory callData = abi.encodeCall(ZcashHeaderRelay.submitHeader, (fx.headers[5]));
        uint256 zeros;
        for (uint256 i; i < callData.length; ++i) {
            if (callData[i] == 0) ++zeros;
        }
        uint256 calldataGas = zeros * 4 + (callData.length - zeros) * 16;
        console.log("submitHeader gas per call: min", minGas, "max", maxGas);
        console.log("submitHeader gas per call: mean", sum / fx.headers.length);
        console.log("calldata bytes", callData.length, "calldata gas", calldataGas);
    }

    // --- structural and contextual rejections ---------------------------------------------------------------------

    function test_revert_unknownParent() public {
        RealFixture memory fx = _loadReal(RECENT);
        ZcashHeaderRelay relay = _deploy(fx);
        _warpTo(fx.headers[1]);
        // Header 4,488,001 before its parent 4,488,000.
        vm.expectRevert(abi.encodeWithSelector(IZcashHeaderRelay.UnknownParent.selector, fx.hashes[0]));
        relay.submitHeader(fx.headers[1]);
    }

    function test_revert_alreadyKnown() public {
        RealFixture memory fx = _loadReal(RECENT);
        ZcashHeaderRelay relay = _deploy(fx);
        _warpTo(fx.headers[0]);
        relay.submitHeader(fx.headers[0]);
        vm.expectRevert(abi.encodeWithSelector(IZcashHeaderRelay.BlockAlreadyKnown.selector, fx.hashes[0]));
        relay.submitHeader(fx.headers[0]);
    }

    function test_revert_wrongLengthAndSolutionSize() public {
        RealFixture memory fx = _loadReal(RECENT);
        ZcashHeaderRelay relay = _deploy(fx);
        bytes memory h = fx.headers[0];
        bytes memory truncated = new bytes(1486);
        for (uint256 i; i < 1486; ++i) {
            truncated[i] = h[i];
        }
        vm.expectRevert(abi.encodeWithSelector(IZcashHeaderRelay.InvalidHeaderLength.selector, 1486));
        relay.submitHeader(truncated);

        bytes memory badSize = _copy(h);
        badSize[141] = 0x3f; // compactSize 0x053f instead of 1344
        vm.expectRevert(IZcashHeaderRelay.InvalidSolutionLength.selector);
        relay.submitHeader(badSize);
    }

    function test_revert_version() public {
        RealFixture memory fx = _loadReal(RECENT);
        ZcashHeaderRelay relay = _deploy(fx);
        _warpTo(fx.headers[0]);
        bytes memory h = _copy(fx.headers[0]);
        _setLe32(h, 0, 3);
        vm.expectRevert(abi.encodeWithSelector(IZcashHeaderRelay.InvalidVersion.selector, int256(3)));
        relay.submitHeader(h);
        // The high bit makes the version negative as an int32 (Zebra rejects it too).
        _setLe32(h, 0, 0x80000004);
        vm.expectRevert(abi.encodeWithSelector(IZcashHeaderRelay.InvalidVersion.selector, int256(int32(-2147483644))));
        relay.submitHeader(h);
    }

    function test_revert_timeNotAfterMedianTimePast() public {
        RealFixture memory fx = _loadReal(RECENT);
        ZcashHeaderRelay relay = _deploy(fx);
        uint256 mtp = _medianOfLast11(fx.contextTimes);
        bytes memory h = _copy(fx.headers[0]);
        _setLe32(h, 100, mtp);
        vm.warp(mtp);
        vm.expectRevert(abi.encodeWithSelector(IZcashHeaderRelay.TimeTooEarly.selector, mtp, mtp));
        relay.submitHeader(h);
    }

    function test_revert_timeTooFarAfterMedianTimePast() public {
        RealFixture memory fx = _loadReal(RECENT);
        ZcashHeaderRelay relay = _deploy(fx);
        uint256 mtp = _medianOfLast11(fx.contextTimes);
        bytes memory h = _copy(fx.headers[0]);
        _setLe32(h, 100, mtp + 90 minutes + 1);
        vm.warp(mtp + 90 minutes);
        vm.expectRevert(
            abi.encodeWithSelector(IZcashHeaderRelay.TimeTooLate.selector, mtp + 90 minutes + 1, mtp + 90 minutes)
        );
        relay.submitHeader(h);
    }

    function test_revert_timeAheadOfBaseClock() public {
        RealFixture memory fx = _loadReal(RECENT);
        ZcashHeaderRelay relay = _deploy(fx);
        uint256 t = _time(fx.headers[0]);
        vm.warp(t - 2 hours - 1);
        vm.expectRevert(abi.encodeWithSelector(IZcashHeaderRelay.TimeTooFarInFuture.selector, t, t - 1));
        relay.submitHeader(fx.headers[0]);
        vm.warp(t - 2 hours);
        relay.submitHeader(fx.headers[0]); // exactly 2 h ahead is still accepted
    }

    /// @notice nBits that differ from the difficulty adjustment are rejected (before the Equihash check).
    function test_revert_wrongBits() public {
        RealFixture memory fx = _loadReal(RECENT);
        ZcashHeaderRelay relay = _deploy(fx);
        _warpTo(fx.headers[0]);
        uint256 bits = _bits(fx.headers[0]);
        bytes memory h = _copy(fx.headers[0]);
        _setLe32(h, 104, bits + 1);
        vm.expectRevert(abi.encodeWithSelector(IZcashHeaderRelay.BadDifficultyBits.selector, bits + 1, bits));
        relay.submitHeader(h);
        // An easier target than the retarget (the Testnet PoWLimit) without the 450 s gap.
        _setLe32(h, 104, 0x2007ffff);
        vm.expectRevert(abi.encodeWithSelector(IZcashHeaderRelay.BadDifficultyBits.selector, 0x2007ffff, bits));
        relay.submitHeader(h);
    }

    /// @notice More than 450 s after its parent, a Testnet block must carry the minimum difficulty.
    function test_revert_minimumDifficultyGapRequiresPowLimitBits() public {
        RealFixture memory fx = _loadReal(RECENT);
        ZcashHeaderRelay relay = _deploy(fx);
        _submitFirst(relay, fx, 3); // R1..R3
        bytes memory h = _copy(fx.headers[3]); // R4, real nBits
        uint256 t = _time(fx.headers[2]) + 451;
        _setLe32(h, 100, t);
        vm.warp(t);
        vm.expectRevert(abi.encodeWithSelector(IZcashHeaderRelay.BadDifficultyBits.selector, _bits(h), 0x2007ffff));
        relay.submitHeader(h);
    }

    // --- Equihash and proof-of-work rejections --------------------------------------------------------------------

    function test_revert_equihash_solutionBitFlip() public {
        _expectEquihashRejection("solution_bit_flip", ZcashEquihash.Result.Collision);
    }

    function test_revert_equihash_nonceBitFlip() public {
        _expectEquihashRejection("nonce_bit_flip", ZcashEquihash.Result.Collision);
    }

    function test_revert_equihash_headerBitFlip() public {
        _expectEquihashRejection("merkle_root_bit_flip", ZcashEquihash.Result.Collision);
    }

    function test_revert_equihash_wrongOrdering() public {
        _expectEquihashRejection("swapped_leaves", ZcashEquihash.Result.OutOfOrder);
        _expectEquihashRejection("swapped_subtrees", ZcashEquihash.Result.OutOfOrder);
    }

    function test_revert_equihash_duplicateIndices() public {
        _expectEquihashRejection("duplicated_half", ZcashEquihash.Result.DuplicateIdxs);
    }

    function _expectEquihashRejection(string memory label, ZcashEquihash.Result kind) internal {
        RealFixture memory fx = _loadReal(RECENT);
        ZcashHeaderRelay relay = _deploy(fx);
        string memory json = _synthetic();
        bytes memory raw = vm.parseJsonBytes(json, string.concat(".mutations.", label, ".raw"));
        string memory expected =
            vm.parseJsonString(json, string.concat(".mutations.", label, ".expected_equihash_error"));
        assertEq(expected, _kindName(kind), "fixture and test disagree on the expected kind");
        _warpTo(raw);
        vm.expectRevert(abi.encodeWithSelector(IZcashHeaderRelay.InvalidEquihashSolution.selector, kind));
        relay.submitHeader(raw);
    }

    /// @notice D1: a genuine Equihash solution meeting the PoWLimit target, but 30 s after its parent, so nBits must
    ///         be the retarget value, not the minimum difficulty.
    function test_revert_validPowButMinDifficultyWithoutGap() public {
        (ZcashHeaderRelay relay, RealFixture memory fx) = _relayAtR3();
        string memory json = _synthetic();
        (bytes memory d1,) = _fork(json, "D1");
        uint256 expected = vm.parseJsonUint(json, ".forks.D1.expected_bits");
        assertEq(expected, _bits(fx.headers[3]), "the retarget value is R4's real nBits");
        vm.expectRevert(abi.encodeWithSelector(IZcashHeaderRelay.BadDifficultyBits.selector, 0x2007ffff, expected));
        relay.submitHeader(d1);
    }

    /// @notice E1: exactly 450 s after its parent does not qualify (the rule is strictly greater).
    function test_revert_validPowButGapOfExactly450s() public {
        (ZcashHeaderRelay relay,) = _relayAtR3();
        string memory json = _synthetic();
        (bytes memory e1,) = _fork(json, "E1");
        vm.expectRevert(
            abi.encodeWithSelector(
                IZcashHeaderRelay.BadDifficultyBits.selector,
                0x2007ffff,
                vm.parseJsonUint(json, ".forks.E1.expected_bits")
            )
        );
        relay.submitHeader(e1);
    }

    /// @notice F1: valid Equihash solution and correct nBits, but the block hash is above the target.
    function test_revert_hashAboveTarget() public {
        (ZcashHeaderRelay relay,) = _relayAtR3();
        (bytes memory f1, bytes32 hash) = _fork(_synthetic(), "F1");
        vm.expectRevert(
            abi.encodeWithSelector(IZcashHeaderRelay.HashAboveTarget.selector, hash, uint256(0x07ffff) << 232)
        );
        relay.submitHeader(f1);
    }

    // --- fork choice --------------------------------------------------------------------------------------------

    /// @notice A minimum-difficulty fork block is far lighter than a real block: no reorganisation.
    function test_fork_lighterForkDoesNotReorg() public {
        RealFixture memory fx = _loadReal(RECENT);
        ZcashHeaderRelay relay = _deploy(fx);
        _submitFirst(relay, fx, 4); // R1..R4
        (bytes memory a1, bytes32 a1Hash) = _fork(_synthetic(), "A1");
        vm.warp(_time(a1) + 60);
        vm.recordLogs();
        relay.submitHeader(a1); // valid, stored as a side branch
        assertEq(vm.getRecordedLogs().length, 1, "only HeaderAdded, no BestChainUpdated");
        (bytes32 tip,,) = relay.bestChainTip();
        assertEq(tip, fx.hashes[3]);
        assertFalse(relay.isInBestChain(a1Hash));
        assertEq(relay.confirmations(a1Hash), 0);
        assertEq(relay.confirmationWork(a1Hash), 0);
        assertTrue(relay.headerOf(a1Hash).known);
    }

    /// @notice Two synthetic branches on R3: A (7 blocks) becomes best, B stays lighter until it is heavier
    ///         (8 blocks), then the real R4 outweighs both.
    function test_fork_heavierForkReorganizesAndLighterDoesNot() public {
        (ZcashHeaderRelay relay, RealFixture memory fx) = _relayAtR3();
        string memory json = _synthetic();
        bytes32 r3 = fx.hashes[2];
        uint256 h3 = fx.heights[2];
        vm.warp(vm.parseJsonUint(json, ".forks.B8.time") + 60);

        // Branch A extends the tip, block by block.
        bytes32[8] memory a;
        for (uint256 i = 1; i <= 7; ++i) {
            bytes memory raw;
            (raw, a[i]) = _fork(json, string.concat("A", vm.toString(i)));
            vm.expectEmit(true, true, false, true, address(relay));
            emit IZcashHeaderRelay.BestChainUpdated(a[i], h3 + i, i == 1 ? r3 : a[i - 1], h3 + i - 1);
            relay.submitHeader(raw);
        }
        (,, uint256 workR3) = _tipOf(relay, r3);
        assertEq(relay.confirmationWork(r3), 7 * MIN_DIFFICULTY_WORK);

        // Branch B: B1..B6 are lighter, B7 ties (the first-seen tip stays), B8 is heavier and reorganizes.
        bytes32[9] memory b;
        for (uint256 i = 1; i <= 7; ++i) {
            bytes memory raw;
            (raw, b[i]) = _fork(json, string.concat("B", vm.toString(i)));
            vm.recordLogs();
            relay.submitHeader(raw);
            assertEq(vm.getRecordedLogs().length, 1, "no BestChainUpdated");
            (bytes32 tip,,) = relay.bestChainTip();
            assertEq(tip, a[7]);
            assertFalse(relay.isInBestChain(b[i]));
            assertEq(relay.headerOf(b[i]).chainWork, workR3 + i * MIN_DIFFICULTY_WORK);
        }
        (bytes memory b8raw, bytes32 b8) = _fork(json, "B8");
        vm.expectEmit(true, true, false, true, address(relay));
        emit IZcashHeaderRelay.BestChainUpdated(b8, h3 + 8, a[7], h3);
        relay.submitHeader(b8raw);
        for (uint256 i = 1; i <= 7; ++i) {
            assertFalse(relay.isInBestChain(a[i]), "A left the best chain");
            assertTrue(relay.isInBestChain(b[i]), "B joined the best chain");
            assertEq(relay.bestBlockHashAt(h3 + i), b[i]);
        }
        assertEq(relay.confirmations(r3), 9);
        assertEq(relay.confirmationWork(r3), 8 * MIN_DIFFICULTY_WORK);

        // The real R4 (parent R3) carries ~10^4 times the work of a minimum-difficulty block: reorg back.
        vm.expectEmit(true, true, false, true, address(relay));
        emit IZcashHeaderRelay.BestChainUpdated(fx.hashes[3], h3 + 1, b8, h3);
        relay.submitHeader(fx.headers[3]);
        for (uint256 i = 2; i <= 8; ++i) {
            assertEq(relay.bestBlockHashAt(h3 + i), bytes32(0), "heights above the new tip are cleared");
        }
        assertFalse(relay.isInBestChain(b8));
        assertFalse(relay.isInBestChain(b[1]));
        assertEq(relay.confirmations(r3), 2);

        // The real chain continues normally.
        for (uint256 i = 4; i < fx.headers.length; ++i) {
            _warpTo(fx.headers[i]);
            relay.submitHeader(fx.headers[i]);
        }
        (bytes32 tip2, uint256 height2,) = relay.bestChainTip();
        assertEq(tip2, fx.hashes[fx.hashes.length - 1]);
        assertEq(height2, fx.heights[fx.heights.length - 1]);
    }

    /// @notice A block on a side branch is checked against its own branch's history, not the best chain's: the
    ///         median-time-past of a child of B6 (while A is best) is B1's time (R3 + 600 s), where the best chain
    ///         would give A1's (R3 + 451 s).
    function test_fork_sideBranchUsesItsOwnHistory() public {
        (ZcashHeaderRelay relay,) = _relayAtR3();
        string memory json = _synthetic();
        vm.warp(vm.parseJsonUint(json, ".forks.B8.time") + 60);
        for (uint256 i = 1; i <= 7; ++i) {
            (bytes memory raw,) = _fork(json, string.concat("A", vm.toString(i)));
            relay.submitHeader(raw);
        }
        bytes32 b6;
        for (uint256 i = 1; i <= 6; ++i) {
            bytes memory raw;
            (raw, b6) = _fork(json, string.concat("B", vm.toString(i)));
            relay.submitHeader(raw);
        }
        uint256 mtp = vm.parseJsonUint(json, ".forks.B7.median_time_past");
        assertEq(mtp, vm.parseJsonUint(json, ".forks.B1.time"));
        (bytes memory b7,) = _fork(json, "B7");
        bytes memory early = _copy(b7);
        _setLe32(early, 100, mtp);
        vm.expectRevert(abi.encodeWithSelector(IZcashHeaderRelay.TimeTooEarly.selector, mtp, mtp));
        relay.submitHeader(early);
        relay.submitHeader(b7); // the real B7 (time B6 + 500 s) is accepted on the side branch
        assertTrue(relay.headerOf(vm.parseJsonBytes32(json, ".forks.B7.hash")).known);
        assertEq(relay.headerOf(vm.parseJsonBytes32(json, ".forks.B7.hash")).prevHash, b6);
    }

    // --- constructor --------------------------------------------------------------------------------------------

    function test_constructor_rejectsBadInput() public {
        RealFixture memory fx = _loadReal(RECENT);
        vm.expectRevert(IZcashHeaderRelay.InvalidCheckpoint.selector);
        new ZcashHeaderRelay(bytes32(0), fx.checkpointHeight, fx.contextTimes, fx.contextBits);
        vm.expectRevert(IZcashHeaderRelay.InvalidCheckpoint.selector);
        new ZcashHeaderRelay(fx.checkpointHash, 653_605, fx.contextTimes, fx.contextBits);

        uint32[] memory short = new uint32[](112);
        vm.expectRevert(IZcashHeaderRelay.InvalidContextLength.selector);
        new ZcashHeaderRelay(fx.checkpointHash, fx.checkpointHeight, short, fx.contextBits);

        uint32[] memory bits = fx.contextBits;
        bits[7] = 0x2008ffff; // above PoWLimit
        vm.expectRevert(abi.encodeWithSelector(IZcashHeaderRelay.InvalidContextBits.selector, 7));
        new ZcashHeaderRelay(fx.checkpointHash, fx.checkpointHeight, fx.contextTimes, bits);
        bits[7] = 0x1e812345; // sign bit set: negative
        vm.expectRevert(abi.encodeWithSelector(IZcashHeaderRelay.InvalidContextBits.selector, 7));
        new ZcashHeaderRelay(fx.checkpointHash, fx.checkpointHeight, fx.contextTimes, bits);
        bits[7] = 0x1e000000; // zero
        vm.expectRevert(abi.encodeWithSelector(IZcashHeaderRelay.InvalidContextBits.selector, 7));
        new ZcashHeaderRelay(fx.checkpointHash, fx.checkpointHeight, fx.contextTimes, bits);
    }

    // --- helpers ------------------------------------------------------------------------------------------------

    function _relayAtR3() internal returns (ZcashHeaderRelay relay, RealFixture memory fx) {
        fx = _loadReal(RECENT);
        relay = _deploy(fx);
        _submitFirst(relay, fx, 3);
        vm.warp(_time(fx.headers[2]) + 1 hours);
    }

    function _submitFirst(ZcashHeaderRelay relay, RealFixture memory fx, uint256 count) internal {
        for (uint256 i; i < count; ++i) {
            _warpTo(fx.headers[i]);
            relay.submitHeader(fx.headers[i]);
        }
    }

    function _tipOf(ZcashHeaderRelay relay, bytes32 h) internal view returns (bytes32, uint256, uint256) {
        IZcashHeaderRelay.HeaderInfo memory info = relay.headerOf(h);
        return (h, info.height, info.chainWork);
    }

    function _medianOfLast11(uint32[] memory times) internal pure returns (uint256) {
        uint256[] memory last = new uint256[](11);
        for (uint256 i; i < 11; ++i) {
            last[i] = times[times.length - 1 - i];
        }
        return ZcashPow.median(last);
    }

    function _kindName(ZcashEquihash.Result kind) internal pure returns (string memory) {
        if (kind == ZcashEquihash.Result.Collision) return "Collision";
        if (kind == ZcashEquihash.Result.OutOfOrder) return "OutOfOrder";
        if (kind == ZcashEquihash.Result.DuplicateIdxs) return "DuplicateIdxs";
        if (kind == ZcashEquihash.Result.NonZeroRootHash) return "NonZeroRootHash";
        return "other";
    }
}
