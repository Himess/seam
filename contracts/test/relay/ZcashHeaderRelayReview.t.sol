// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {RelayFixtures} from "./RelayFixtures.sol";
import {ZcashHeaderRelay} from "../../src/relay/ZcashHeaderRelay.sol";
import {ZcashPow} from "../../src/relay/ZcashPow.sol";

/// @notice Regressions for the contracts review of 2026-10-11 (docs/SECURITY_REVIEW.md, "Contracts"): R-1 (a reorg
///         used to delete every orphaned height, so a long, light attacker branch on a stale tip could push the honest
///         reorganisation above the transaction gas limit and freeze the relay) and I-2 (batches with a known header).
///         The first 8 attacker blocks are genuine minimum-difficulty headers (real Equihash solutions, synthetic
///         fixture); longer branches are written with vm.store as stand-ins, exactly what submitHeader would store.
contract ZcashHeaderRelayReviewTest is RelayFixtures {
    uint256 internal constant TX_GAS_CAP = 16_777_216;
    uint256 internal constant SLOT_BLOCKS = 0;
    uint256 internal constant SLOT_BEST_HASH_AT = 1;
    uint256 internal constant SLOT_BEST_TIP = 4;
    uint256 internal constant MIN_DIFFICULTY_WORK = 32;

    /// Real R1..R3, then the attacker's genuine minimum-difficulty blocks B1..B8 extend the (stale) tip R3.
    function _relayWithGenuineAttackerBlocks()
        internal
        returns (ZcashHeaderRelay relay, RealFixture memory fx, bytes32 b8, uint256 h8, uint256 work8)
    {
        fx = _loadReal(RECENT);
        relay = _deploy(fx);
        for (uint256 i; i < 3; ++i) {
            _warpTo(fx.headers[i]);
            relay.submitHeader(fx.headers[i]);
        }
        string memory json = _synthetic();
        vm.warp(vm.parseJsonUint(json, ".forks.B8.time") + 60);
        for (uint256 i = 1; i <= 8; ++i) {
            bytes memory raw;
            (raw, b8) = _fork(json, string.concat("B", vm.toString(i)));
            relay.submitHeader(raw);
        }
        bytes32 tip;
        (tip, h8, work8) = relay.bestChainTip();
        assertEq(tip, b8, "the attacker's blocks are the best chain");
    }

    /// `count` further cheap attacker blocks above `fromHeight`: best-chain entries and the tip record.
    function _extendWithStandIns(ZcashHeaderRelay relay, uint256 fromHeight, uint256 fromWork, uint256 count)
        internal
        returns (bytes32 tip)
    {
        for (uint256 i = 1; i <= count; ++i) {
            tip = keccak256(abi.encode("review.standIn", i));
            vm.store(address(relay), keccak256(abi.encode(fromHeight + i, SLOT_BEST_HASH_AT)), tip);
        }
        bytes32 rec = keccak256(abi.encode(tip, SLOT_BLOCKS));
        uint256 packed = (fromWork + MIN_DIFFICULTY_WORK * count) | ((fromHeight + count) << 128) | (uint256(1) << 160)
            | (uint256(ZcashPow.encodeBits(0x07ffff << 232)) << 192);
        vm.store(address(relay), bytes32(uint256(rec) + 1), bytes32(packed));
        vm.store(address(relay), bytes32(SLOT_BEST_TIP), tip);
    }

    function _honestReorgGas(uint256 extra) internal returns (uint256 used, uint256 orphaned) {
        (ZcashHeaderRelay relay, RealFixture memory fx,, uint256 h8, uint256 work8) = _relayWithGenuineAttackerBlocks();
        if (extra != 0) _extendWithStandIns(relay, h8, work8, extra);
        (, uint256 attackerHeight,) = relay.bestChainTip();
        orphaned = attackerHeight - fx.heights[3];
        uint256 g = gasleft();
        relay.submitHeader(fx.headers[3]);
        used = g - gasleft();
        (bytes32 tip,,) = relay.bestChainTip();
        assertEq(tip, fx.hashes[3], "reorganised back to the honest chain");
    }

    /// R-1: reorganising back from 3,400 orphaned heights costs the same as from 8, far below the gas limit.
    function test_reorgCostDoesNotDependOnTheOrphanedBranchLength() public {
        (uint256 gShort,) = _honestReorgGas(0);
        (uint256 gLong, uint256 orphaned) = _honestReorgGas(3_392);
        assertGt(orphaned, 3_000);
        assertLt(gLong, gShort + 50_000, "no per-orphaned-height cost");
        assertLt(gLong, TX_GAS_CAP / 4);
    }

    /// R-1: entries above the tip are leftovers of the orphaned branch; every view ignores them, and the honest chain
    /// keeps extending.
    function test_staleEntriesAboveTheTipAreIgnored() public {
        (ZcashHeaderRelay relay, RealFixture memory fx, bytes32 b8, uint256 h8,) = _relayWithGenuineAttackerBlocks();
        relay.submitHeader(fx.headers[3]); // heavier than the whole attacker branch
        (bytes32 tip, uint256 tipHeight,) = relay.bestChainTip();
        assertEq(tip, fx.hashes[3]);
        assertLt(tipHeight, h8);
        assertFalse(relay.isInBestChain(b8));
        assertEq(relay.confirmations(b8), 0);
        assertEq(relay.confirmationWork(b8), 0);
        assertEq(relay.bestBlockHashAt(h8), bytes32(0));

        _warpTo(fx.headers[4]);
        relay.submitHeader(fx.headers[4]);
        (tip,,) = relay.bestChainTip();
        assertEq(tip, fx.hashes[4]);
        assertTrue(relay.isInBestChain(fx.hashes[3]));
        assertEq(relay.confirmations(fx.hashes[3]), 2);
    }

    /// I-2: a header someone else submitted first no longer makes a batch fail.
    function test_batchSkipsHeadersAlreadyKnown() public {
        RealFixture memory fx = _loadReal(RECENT);
        ZcashHeaderRelay relay = _deploy(fx);
        vm.warp(_time(fx.headers[fx.headers.length - 1]) + 60);
        relay.submitHeader(fx.headers[0]);
        relay.submitHeaders(fx.headers);
        (bytes32 tip,,) = relay.bestChainTip();
        assertEq(tip, fx.hashes[fx.hashes.length - 1]);
    }
}
