// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {console} from "forge-std/console.sol";
import {RelayFixtures} from "./RelayFixtures.sol";
import {ZcashHeaderRelay} from "../../src/relay/ZcashHeaderRelay.sol";
import {IZcashHeaderRelay, ZCASH_MAINNET} from "../../src/relay/IZcashHeaderRelay.sol";
import {ZcashEquihash} from "../../src/relay/ZcashEquihash.sol";
import {ZcashPow} from "../../src/relay/ZcashPow.sol";

/// @notice ZcashHeaderRelay with the Mainnet parameters against 180 real Zcash Mainnet headers (3,497,560 ..
///         3,497,739, pinned by Zebra's checkpoint list and cross-checked with CipherScan, see src/relay/README.md),
///         and where the Testnet and Mainnet rules differ, each network's relay refusing the other's headers.
contract ZcashHeaderRelayMainnetTest is RelayFixtures {
    /// Index of 3,497,585 in the fixture: 451 s after its parent, one second over the Testnet threshold.
    uint256 internal constant GAP_451 = 25;
    /// Index of 3,497,733: 466 s after its parent.
    uint256 internal constant GAP_466 = 173;

    // --- the Mainnet chain under the Mainnet rules --------------------------------------------------------------

    function test_mainnet_networkAndParameters() public {
        RealFixture memory fx = _loadReal(MAINNET_RECENT);
        ZcashHeaderRelay relay = _deploy(fx);
        assertEq(relay.networkId(), ZCASH_MAINNET);
        assertEq(relay.networkId(), keccak256("zcash-mainnet"));
        assertEq(abi.encode(relay.params()), abi.encode(_mainnetParams()));
        (bytes32 tip, uint256 height,) = relay.bestChainTip();
        assertEq(tip, fx.checkpointHash);
        assertEq(height, 3_497_559);
    }

    /// @notice 180 consecutive real Mainnet headers, each verified in full (75 s spacing, 17-block window, Mainnet
    ///         PoWLimit, no minimum-difficulty rule), including two blocks more than 450 s after their parents.
    function test_mainnet_realHeadersAccepted() public {
        RealFixture memory fx = _loadReal(MAINNET_RECENT);
        assertEq(fx.headers.length, 180);
        assertEq(fx.heights[GAP_451], 3_497_585);
        assertEq(_time(fx.headers[GAP_451]) - _time(fx.headers[GAP_451 - 1]), 451);
        assertEq(fx.heights[GAP_466], 3_497_733);
        assertEq(_time(fx.headers[GAP_466]) - _time(fx.headers[GAP_466 - 1]), 466);

        ZcashHeaderRelay relay = _deploy(fx);
        uint256 n = fx.headers.length;
        for (uint256 i; i < n; ++i) {
            _warpTo(fx.headers[i]);
            vm.expectEmit(true, true, false, false, address(relay));
            emit IZcashHeaderRelay.HeaderAdded(fx.hashes[i], i == 0 ? fx.checkpointHash : fx.hashes[i - 1], 0, 0);
            assertEq(relay.submitHeader(fx.headers[i]), fx.hashes[i], "hash");
            (bytes32 tip, uint256 height,) = relay.bestChainTip();
            assertEq(tip, fx.hashes[i], "tip");
            assertEq(height, fx.heights[i], "height");
        }
        assertEq(relay.confirmations(fx.checkpointHash), n + 1);
        assertEq(relay.confirmations(fx.hashes[GAP_451]), n - GAP_451);
        for (uint256 i; i < n; ++i) {
            assertEq(relay.bestBlockHashAt(fx.heights[i]), fx.hashes[i]);
        }
        // The blocks after a long gap carry the ordinary retarget value, far below the Mainnet PoWLimit.
        assertEq(relay.headerOf(fx.hashes[GAP_451]).bits, 0x1b7b1e0e);
        assertEq(relay.headerOf(fx.hashes[GAP_466]).bits, 0x1b77ebf9);
        // Cumulative work is the sum of the blocks' work.
        uint256 sum;
        for (uint256 i; i < n; ++i) {
            (uint256 target,) = ZcashPow.decodeBits(_bits(fx.headers[i]));
            sum += ZcashPow.work(target);
        }
        (,, uint256 tipWork) = relay.bestChainTip();
        assertEq(tipWork, sum);
        assertEq(relay.confirmationWork(fx.checkpointHash), sum);
    }

    function test_mainnet_submitHeadersBatch() public {
        RealFixture memory fx = _loadReal(MAINNET_RECENT);
        ZcashHeaderRelay relay = _deploy(fx);
        bytes[] memory batch = new bytes[](20);
        for (uint256 i; i < 20; ++i) {
            batch[i] = fx.headers[i];
        }
        vm.warp(_time(fx.headers[19]) + 60);
        relay.submitHeaders(batch);
        (bytes32 tip, uint256 height,) = relay.bestChainTip();
        assertEq(tip, fx.hashes[19]);
        assertEq(height, fx.heights[19]);
    }

    /// @notice Gas per Mainnet header (17-block window before NU7). Run with `--isolate` for whole transactions.
    function test_mainnet_gas_submitOneHeader() public {
        RealFixture memory fx = _loadReal(MAINNET_RECENT);
        ZcashHeaderRelay relay = _deploy(fx);
        uint256 sum;
        uint256 maxGas;
        uint256 minGas = type(uint256).max;
        for (uint256 i; i < 32; ++i) {
            _warpTo(fx.headers[i]);
            uint256 g = gasleft();
            relay.submitHeader(fx.headers[i]);
            uint256 measured = g - gasleft();
            sum += measured;
            if (measured > maxGas) maxGas = measured;
            if (measured < minGas) minGas = measured;
        }
        console.log("Mainnet submitHeader gas per call: min", minGas, "max", maxGas);
        console.log("Mainnet submitHeader gas per call: mean", sum / 32);
    }

    // --- rejections under the Mainnet rules ---------------------------------------------------------------------

    function test_mainnet_revert_wrongBits() public {
        RealFixture memory fx = _loadReal(MAINNET_RECENT);
        ZcashHeaderRelay relay = _deploy(fx);
        _warpTo(fx.headers[0]);
        uint256 bits = _bits(fx.headers[0]);
        bytes memory h = _copy(fx.headers[0]);
        _setLe32(h, 104, bits + 1);
        vm.expectRevert(abi.encodeWithSelector(IZcashHeaderRelay.BadDifficultyBits.selector, bits + 1, bits));
        relay.submitHeader(h);
        _setLe32(h, 104, 0x1f07ffff); // the Mainnet PoWLimit
        vm.expectRevert(abi.encodeWithSelector(IZcashHeaderRelay.BadDifficultyBits.selector, 0x1f07ffff, bits));
        relay.submitHeader(h);
    }

    /// @notice No minimum-difficulty rule on Mainnet: 451 s after its parent, a block still needs the retarget value.
    function test_mainnet_revert_powLimitBitsAfterLongGap() public {
        RealFixture memory fx = _loadReal(MAINNET_RECENT);
        ZcashHeaderRelay relay = _deploy(fx);
        _submitFirst(relay, fx, GAP_451);
        bytes memory h = _copy(fx.headers[GAP_451]);
        _setLe32(h, 104, 0x1f07ffff);
        _warpTo(h);
        vm.expectRevert(abi.encodeWithSelector(IZcashHeaderRelay.BadDifficultyBits.selector, 0x1f07ffff, 0x1b7b1e0e));
        relay.submitHeader(h);
        relay.submitHeader(fx.headers[GAP_451]); // the real block
    }

    function test_mainnet_revert_timeNotAfterMedianTimePast() public {
        RealFixture memory fx = _loadReal(MAINNET_RECENT);
        ZcashHeaderRelay relay = _deploy(fx);
        uint256 mtp = _medianOfLast11(fx.contextTimes);
        bytes memory h = _copy(fx.headers[0]);
        _setLe32(h, 100, mtp);
        vm.warp(mtp);
        vm.expectRevert(abi.encodeWithSelector(IZcashHeaderRelay.TimeTooEarly.selector, mtp, mtp));
        relay.submitHeader(h);
    }

    /// @notice The 90-minute rule applies on Mainnet (at every height).
    function test_mainnet_revert_timeTooFarAfterMedianTimePast() public {
        RealFixture memory fx = _loadReal(MAINNET_RECENT);
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

    function test_mainnet_revert_timeAheadOfBaseClock() public {
        RealFixture memory fx = _loadReal(MAINNET_RECENT);
        ZcashHeaderRelay relay = _deploy(fx);
        uint256 t = _time(fx.headers[0]);
        vm.warp(t - 2 hours - 1);
        vm.expectRevert(abi.encodeWithSelector(IZcashHeaderRelay.TimeTooFarInFuture.selector, t, t - 1));
        relay.submitHeader(fx.headers[0]);
        vm.warp(t - 2 hours);
        relay.submitHeader(fx.headers[0]);
    }

    /// @notice One-defect copies of the real header 3,497,560, each with the reference's error kind.
    function test_mainnet_revert_badEquihash() public {
        ZcashHeaderRelay relay = _deploy(_loadReal(MAINNET_RECENT));
        string memory json = vm.readFile(MAINNET_SYNTHETIC);
        _expectEquihashRejection(relay, json, "solution_bit_flip", ZcashEquihash.Result.Collision);
        _expectEquihashRejection(relay, json, "nonce_bit_flip", ZcashEquihash.Result.Collision);
        _expectEquihashRejection(relay, json, "merkle_root_bit_flip", ZcashEquihash.Result.Collision);
        _expectEquihashRejection(relay, json, "swapped_leaves", ZcashEquihash.Result.OutOfOrder);
        _expectEquihashRejection(relay, json, "swapped_subtrees", ZcashEquihash.Result.OutOfOrder);
        _expectEquihashRejection(relay, json, "duplicated_half", ZcashEquihash.Result.DuplicateIdxs);
    }

    function _expectEquihashRejection(
        ZcashHeaderRelay relay,
        string memory json,
        string memory label,
        ZcashEquihash.Result kind
    ) internal {
        bytes memory raw = vm.parseJsonBytes(json, string.concat(".mutations.", label, ".raw"));
        assertEq(
            keccak256(bytes(vm.parseJsonString(json, string.concat(".mutations.", label, ".expected_equihash_error")))),
            keccak256(bytes(_kindName(kind))),
            "fixture and test disagree on the expected kind"
        );
        _warpTo(raw);
        vm.expectRevert(abi.encodeWithSelector(IZcashHeaderRelay.InvalidEquihashSolution.selector, kind));
        relay.submitHeader(raw);
    }

    /// @notice F1: on the real block 3,497,562, the real child's time and nBits and a genuine Equihash solution, but
    ///         a block hash above the Mainnet target.
    function test_mainnet_revert_hashAboveTarget() public {
        RealFixture memory fx = _loadReal(MAINNET_RECENT);
        ZcashHeaderRelay relay = _deploy(fx);
        _submitFirst(relay, fx, 3);
        string memory json = vm.readFile(MAINNET_SYNTHETIC);
        (bytes memory f1, bytes32 hash) = _fork(json, "F1");
        assertEq(vm.parseJsonBytes32(json, ".fork_base.hash"), fx.hashes[2]);
        assertEq(_bits(f1), _bits(fx.headers[3]), "the retarget value");
        (uint256 target,) = ZcashPow.decodeBits(_bits(f1));
        _warpTo(f1);
        vm.expectRevert(abi.encodeWithSelector(IZcashHeaderRelay.HashAboveTarget.selector, hash, target));
        relay.submitHeader(f1);
        relay.submitHeader(fx.headers[3]); // the real child is accepted
    }

    // --- where the networks' rules differ -----------------------------------------------------------------------

    /// @notice The header rules alone do not tell the networks apart where they agree (the first 25 Mainnet headers
    ///         pass the Testnet rules too: same 75 s / 17-block rules before NU7, targets under both PoWLimits); the
    ///         network id does. Where they differ, a Testnet relay refuses Mainnet blocks: 3,497,585 came 451 s after
    ///         its parent, so the Testnet rule demands the minimum difficulty.
    function test_testnetRulesRefuseMainnetBlockAfterLongGap() public {
        RealFixture memory fx = _loadReal(MAINNET_RECENT);
        ZcashHeaderRelay testnetRelay = _deploy(fx, _testnetParams());
        _submitFirst(testnetRelay, fx, GAP_451);
        _warpTo(fx.headers[GAP_451]);
        vm.expectRevert(abi.encodeWithSelector(IZcashHeaderRelay.BadDifficultyBits.selector, 0x1b7b1e0e, 0x2007ffff));
        testnetRelay.submitHeader(fx.headers[GAP_451]);

        ZcashHeaderRelay mainnetRelay = _deploy(fx);
        _submitFirst(mainnetRelay, fx, GAP_451 + 1);
        (bytes32 tip,,) = mainnetRelay.bestChainTip();
        assertEq(tip, fx.hashes[GAP_451]);
    }

    /// @notice A genuine Testnet minimum-difficulty block (synthetic A1: 451 s after R3, nBits 0x2007ffff, real
    ///         Equihash solution) is accepted under the Testnet rules and refused without the minimum-difficulty
    ///         rule, which then demands the retarget value (the real R4's nBits).
    function test_withoutMinDifficultyRuleTestnetMinDifficultyBlockIsRefused() public {
        RealFixture memory fx = _loadReal(RECENT);
        string memory json = _synthetic();
        (bytes memory a1,) = _fork(json, "A1");

        ZcashHeaderRelay.RelayParams memory p = _testnetParams();
        p.networkId = keccak256("zcash-testnet without the minimum-difficulty rule");
        p.minDifficultyBlocks = false;
        p.minDifficultyStartHeight = 0;
        ZcashHeaderRelay relay = _deploy(fx, p);
        _submitFirst(relay, fx, 3);
        vm.warp(_time(a1) + 60);
        vm.expectRevert(
            abi.encodeWithSelector(IZcashHeaderRelay.BadDifficultyBits.selector, 0x2007ffff, _bits(fx.headers[3]))
        );
        relay.submitHeader(a1);

        ZcashHeaderRelay testnetRelay = _deploy(fx);
        _submitFirst(testnetRelay, fx, 3);
        vm.warp(_time(a1) + 60);
        testnetRelay.submitHeader(a1);
    }

    /// @notice Mainnet rules refuse a real Testnet header from between the two networks' NU7 heights: block 4,465,015
    ///         comes before NU7 on Testnet (75 s, 17-block window) but after Mainnet's NU7 at 3,543,405, where the
    ///         relay applies 25 s and 102 blocks (value from the Python model, tools/zcash_header_check.py).
    function test_mainnetRulesRefuseTestnetHeaders() public {
        RealFixture memory fx = _loadReal(NU7_ACTIVATION);
        ZcashHeaderRelay relay = _deploy(fx, _mainnetParams());
        _warpTo(fx.headers[0]);
        vm.expectRevert(abi.encodeWithSelector(IZcashHeaderRelay.BadDifficultyBits.selector, 0x1f0098bd, 0x1f01a82a));
        relay.submitHeader(fx.headers[0]);
        _deploy(fx).submitHeader(fx.headers[0]); // the Testnet relay accepts it
    }

    /// @notice Once both networks are past NU7 they share the adjustment (ZIP 218): Mainnet rules compute the nBits of
    ///         a real post-NU7 Testnet header exactly (value from the Python model). Only the PoW limit and Testnet's
    ///         minimum-difficulty rule still differ, so the network a relay follows is fixed by its checkpoint, which
    ///         is trusted deployment input: DeployRelay refuses a checkpoint file of another network
    ///         (test_config_refusesACheckpointOfAnotherNetwork).
    function test_afterBothNu7sTheAdjustmentIsShared() public {
        RealFixture memory fx = _loadReal(RECENT);
        ZcashHeaderRelay relay = _deploy(fx, _mainnetParams());
        _warpTo(fx.headers[0]);
        assertEq(relay.submitHeader(fx.headers[0]), fx.hashes[0]);
    }

    /// @notice The Testnet minimum difficulty (0x2007ffff) is above the Mainnet PoWLimit: a Mainnet relay refuses a
    ///         context that holds a Testnet minimum-difficulty block; a Testnet relay accepts it.
    function test_mainnetPowLimitRefusesTestnetMinimumDifficultyContext() public {
        RealFixture memory fx = _loadReal(RECENT);
        fx.contextBits[40] = 0x2007ffff;
        _deploy(fx, _testnetParams());
        ZcashHeaderRelay.RelayParams memory p = _mainnetParams();
        vm.expectRevert(abi.encodeWithSelector(IZcashHeaderRelay.InvalidContextBits.selector, 40));
        this.deployRelay(p, fx.checkpointHash, fx.checkpointHeight, fx.contextTimes, fx.contextBits);
        fx.contextBits[40] = 0x1f07ffff; // the Mainnet PoWLimit itself is allowed
        assertEq(_deploy(fx, p).networkId(), ZCASH_MAINNET);
    }

    /// @notice Mainnet's NU7 is at 3,543,405 (ZIP 259). If a relay assumed it at 3,497,560, the real pre-NU7 block
    ///         there would fail the 25 s / 102-block adjustment (value from the Python model).
    function test_mainnet_nu7HeightDecidesTheAdjustment() public {
        RealFixture memory fx = _loadReal(MAINNET_RECENT);
        ZcashHeaderRelay.RelayParams memory p = _mainnetParams();
        p.nu7ActivationHeight = 3_497_560;
        ZcashHeaderRelay relay = _deploy(fx, p);
        _warpTo(fx.headers[0]);
        vm.expectRevert(abi.encodeWithSelector(IZcashHeaderRelay.BadDifficultyBits.selector, 0x1c0094c3, 0x1c00b01b));
        relay.submitHeader(fx.headers[0]);
    }

    // --- helpers ------------------------------------------------------------------------------------------------

    function _submitFirst(ZcashHeaderRelay relay, RealFixture memory fx, uint256 count) internal {
        for (uint256 i; i < count; ++i) {
            _warpTo(fx.headers[i]);
            relay.submitHeader(fx.headers[i]);
        }
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
