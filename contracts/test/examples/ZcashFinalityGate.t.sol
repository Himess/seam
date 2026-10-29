// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {RelayFixtures} from "../relay/RelayFixtures.sol";
import {ZcashHeaderRelay} from "../../src/relay/ZcashHeaderRelay.sol";
import {ZCASH_MAINNET, ZCASH_TESTNET} from "../../src/relay/IZcashHeaderRelay.sol";
import {IZcashHeaderRelayReader} from "../../src/interfaces/IZcashHeaderRelayReader.sol";
import {ZcashFinalityGate} from "../../src/examples/ZcashFinalityGate.sol";

/// @notice Exposes the gate's internal check.
contract FinalityGateHarness is ZcashFinalityGate {
    constructor(IZcashHeaderRelayReader relay_, bytes32 expectedNetworkId, uint256 minWork_)
        ZcashFinalityGate(relay_, expectedNetworkId, minWork_)
    {}

    function requireFinal(bytes32 blockHash) external view {
        _requireFinal(blockHash);
    }
}

/// @notice ZcashFinalityGate on a relay fed with real Testnet headers and the synthetic forks of testnet_synthetic.json.
contract ZcashFinalityGateTest is RelayFixtures {
    uint256 internal constant MIN_DIFFICULTY_WORK = 32; // work of nBits 0x2007ffff

    /// @notice Deploys a gate in an external call: tests expecting a constructor revert call `this.deployGate`.
    function deployGate(IZcashHeaderRelayReader relay, bytes32 expectedNetworkId, uint256 minWork)
        external
        returns (FinalityGateHarness)
    {
        return new FinalityGateHarness(relay, expectedNetworkId, minWork);
    }

    function test_constructor_refusesZeroMinWorkAndAnotherNetwork() public {
        IZcashHeaderRelayReader relay = _reader(_deploy(_loadReal(RECENT)));
        vm.expectRevert(ZcashFinalityGate.ZeroMinWork.selector);
        this.deployGate(relay, ZCASH_TESTNET, 0);
        vm.expectRevert(abi.encodeWithSelector(ZcashFinalityGate.WrongNetwork.selector, ZCASH_TESTNET));
        this.deployGate(relay, ZCASH_MAINNET, 1);

        FinalityGateHarness gate = this.deployGate(relay, ZCASH_TESTNET, 1);
        assertEq(address(gate.relay()), address(relay));
        assertEq(gate.minWork(), 1);
    }

    function test_unknownHashIsNotFinal() public {
        (ZcashHeaderRelay relay,) = _fedRelay();
        FinalityGateHarness gate = this.deployGate(_reader(relay), ZCASH_TESTNET, 1);
        bytes32 unknown = keccak256("unknown");
        assertFalse(gate.isFinal(unknown));
        vm.expectRevert(abi.encodeWithSelector(ZcashFinalityGate.NotFinal.selector, unknown, 0, 1));
        gate.requireFinal(unknown);
    }

    /// @notice Nothing is built on the tip yet: its confirmationWork is 0.
    function test_tipIsNotFinal() public {
        (ZcashHeaderRelay relay, RealFixture memory fx) = _fedRelay();
        bytes32 tip = fx.hashes[fx.hashes.length - 1];
        assertTrue(relay.isInBestChain(tip));
        assertEq(relay.confirmationWork(tip), 0);
        FinalityGateHarness gate = this.deployGate(_reader(relay), ZCASH_TESTNET, 1);
        assertFalse(gate.isFinal(tip));
        vm.expectRevert(abi.encodeWithSelector(ZcashFinalityGate.NotFinal.selector, tip, 0, 1));
        gate.requireFinal(tip);
    }

    /// @notice A block with work W on top of it is final at minWork = W and not at W + 1.
    function test_finalAtExactlyTheWorkOnTop() public {
        (ZcashHeaderRelay relay, RealFixture memory fx) = _fedRelay();
        bytes32 h = fx.hashes[0];
        uint256 w = relay.confirmationWork(h);
        assertGt(w, 0);

        FinalityGateHarness atW = this.deployGate(_reader(relay), ZCASH_TESTNET, w);
        assertTrue(atW.isFinal(h));
        atW.requireFinal(h);

        FinalityGateHarness aboveW = this.deployGate(_reader(relay), ZCASH_TESTNET, w + 1);
        assertFalse(aboveW.isFinal(h));
        vm.expectRevert(abi.encodeWithSelector(ZcashFinalityGate.NotFinal.selector, h, w, w + 1));
        aboveW.requireFinal(h);
    }

    /// @notice A1 is final while the synthetic branch A1..A7 is the best chain (six minimum-difficulty blocks on top of
    ///         it). The real R4 on R3 carries far more work: the relay reorganises, A1 leaves the best chain, and the
    ///         gate no longer counts it as final.
    function test_blockThatLeavesTheBestChainIsNoLongerFinal() public {
        RealFixture memory fx = _loadReal(RECENT);
        ZcashHeaderRelay relay = _deploy(fx);
        for (uint256 i; i < 3; ++i) {
            _warpTo(fx.headers[i]);
            relay.submitHeader(fx.headers[i]); // R1..R3
        }
        string memory json = _synthetic();
        vm.warp(vm.parseJsonUint(json, ".forks.B8.time") + 60);
        bytes32 a1;
        for (uint256 i = 1; i <= 7; ++i) {
            (bytes memory raw, bytes32 hash) = _fork(json, string.concat("A", vm.toString(i)));
            relay.submitHeader(raw);
            if (i == 1) a1 = hash;
        }
        uint256 required = 6 * MIN_DIFFICULTY_WORK;
        FinalityGateHarness gate = this.deployGate(_reader(relay), ZCASH_TESTNET, required);
        assertTrue(gate.isFinal(a1));

        relay.submitHeader(fx.headers[3]); // R4
        assertFalse(relay.isInBestChain(a1));
        assertFalse(gate.isFinal(a1));
        vm.expectRevert(abi.encodeWithSelector(ZcashFinalityGate.NotFinal.selector, a1, 0, required));
        gate.requireFinal(a1);
    }

    // --- helpers ------------------------------------------------------------------------------------------------

    /// @dev A relay from testnet_recent.json's checkpoint, fed with its 32 real headers.
    function _fedRelay() internal returns (ZcashHeaderRelay relay, RealFixture memory fx) {
        fx = _loadReal(RECENT);
        relay = _deploy(fx);
        vm.warp(_time(fx.headers[fx.headers.length - 1]) + 60);
        relay.submitHeaders(fx.headers);
    }

    function _reader(ZcashHeaderRelay relay) internal pure returns (IZcashHeaderRelayReader) {
        return IZcashHeaderRelayReader(address(relay));
    }
}
