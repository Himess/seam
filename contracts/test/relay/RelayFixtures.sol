// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {Test} from "forge-std/Test.sol";
import {ZcashHeaderRelay} from "../../src/relay/ZcashHeaderRelay.sol";
import {ZCASH_MAINNET, ZCASH_TESTNET} from "../../src/relay/IZcashHeaderRelay.sol";

/// @notice Loads the Zcash header fixtures of test/relay/fixtures (sources: src/relay/README.md, "Fixtures") and holds
///         the network parameters every relay test deploys with.
abstract contract RelayFixtures is Test {
    string internal constant RECENT = "test/relay/fixtures/testnet_recent.json";
    string internal constant NU7_ACTIVATION = "test/relay/fixtures/testnet_nu7_activation.json";
    string internal constant SYNTHETIC = "test/relay/fixtures/testnet_synthetic.json";
    string internal constant MAINNET_RECENT = "test/relay/fixtures/mainnet_recent.json";
    string internal constant MAINNET_SYNTHETIC = "test/relay/fixtures/mainnet_synthetic.json";

    struct RealFixture {
        string network; // "Zcash Testnet" or "Zcash Mainnet"
        bytes32 checkpointHash;
        uint256 checkpointHeight;
        uint32[] contextTimes;
        uint32[] contextBits;
        bytes[] headers;
        bytes32[] hashes;
        uint256[] heights;
    }

    /// @notice Zcash Testnet (contracts/config/relay-testnet.json; sources on ZcashHeaderRelay.RelayParams and in
    ///         docs/ref/FACTS.md). Every Testnet test uses exactly these values.
    function _testnetParams() internal pure returns (ZcashHeaderRelay.RelayParams memory) {
        return ZcashHeaderRelay.RelayParams({
            networkId: ZCASH_TESTNET,
            powLimit: uint256(0x07ffff) << 232,
            powLimitBits: 0x2007ffff,
            minDifficultyBlocks: true,
            minDifficultyStartHeight: 299_188,
            maxTimeStartHeight: 653_606,
            nu7ActivationHeight: 4_465_026,
            preNu7TargetSpacing: 75,
            preNu7AveragingWindow: 17,
            postNu7TargetSpacing: 25,
            postNu7AveragingWindow: 102,
            minCheckpointHeight: 653_606
        });
    }

    /// @notice Zcash Mainnet (contracts/config/relay-mainnet.json): PoWLimit 2^243 - 1, no minimum-difficulty rule,
    ///         the 90-minute rule at every height, NU7 not yet assigned, checkpoints from Blossom (653,600).
    function _mainnetParams() internal pure returns (ZcashHeaderRelay.RelayParams memory) {
        return ZcashHeaderRelay.RelayParams({
            networkId: ZCASH_MAINNET,
            powLimit: uint256(0x07ffff) << 224,
            powLimitBits: 0x1f07ffff,
            minDifficultyBlocks: false,
            minDifficultyStartHeight: 0,
            maxTimeStartHeight: 0,
            nu7ActivationHeight: type(uint32).max,
            preNu7TargetSpacing: 75,
            preNu7AveragingWindow: 17,
            postNu7TargetSpacing: 25,
            postNu7AveragingWindow: 102,
            minCheckpointHeight: 653_600
        });
    }

    /// @notice The parameters of the network a fixture comes from.
    function _paramsFor(RealFixture memory fx) internal pure returns (ZcashHeaderRelay.RelayParams memory) {
        bytes32 network = keccak256(bytes(fx.network));
        if (network == keccak256("Zcash Testnet")) return _testnetParams();
        if (network == keccak256("Zcash Mainnet")) return _mainnetParams();
        revert(string.concat("unknown fixture network ", fx.network));
    }

    function _loadReal(string memory path) internal view returns (RealFixture memory fx) {
        string memory json = vm.readFile(path);
        fx.network = vm.parseJsonString(json, ".network");
        fx.checkpointHash = vm.parseJsonBytes32(json, ".checkpoint_hash");
        fx.checkpointHeight = vm.parseJsonUint(json, ".checkpoint_height");
        fx.contextTimes = _toUint32(vm.parseJsonUintArray(json, ".context_times"));
        fx.contextBits = _toUint32(vm.parseJsonUintArray(json, ".context_bits"));
        fx.headers = vm.parseJsonBytesArray(json, ".header_raws");
        fx.hashes = vm.parseJsonBytes32Array(json, ".header_hashes");
        fx.heights = vm.parseJsonUintArray(json, ".header_heights");
    }

    /// @notice A relay from the fixture's checkpoint with the parameters of the fixture's network.
    function _deploy(RealFixture memory fx) internal returns (ZcashHeaderRelay) {
        return _deploy(fx, _paramsFor(fx));
    }

    function _deploy(RealFixture memory fx, ZcashHeaderRelay.RelayParams memory params)
        internal
        returns (ZcashHeaderRelay)
    {
        return new ZcashHeaderRelay(params, fx.checkpointHash, fx.checkpointHeight, fx.contextTimes, fx.contextBits);
    }

    /// @notice Deploys a relay in an external call: tests expecting a constructor revert call `this.deployRelay`.
    /// @dev Foundry 1.8 links tests dynamically by default (`dynamic_test_linking`): `new` in a test becomes
    ///      `vm.deployCode`, and a constructor revert awaited by `vm.expectRevert` then ends the test early, reported
    ///      as a pass, so every check after it is skipped. Behind an external call, expectRevert covers only that call.
    function deployRelay(
        ZcashHeaderRelay.RelayParams memory params,
        bytes32 checkpointHash,
        uint256 checkpointHeight,
        uint32[] memory contextTimes,
        uint32[] memory contextBits
    ) external returns (ZcashHeaderRelay) {
        return new ZcashHeaderRelay(params, checkpointHash, checkpointHeight, contextTimes, contextBits);
    }

    /// @dev Moves Base's clock just past a header's time (the relay rejects headers more than 2 h ahead of it).
    function _warpTo(bytes memory header) internal {
        vm.warp(_time(header) + 60);
    }

    function _synthetic() internal view returns (string memory) {
        return vm.readFile(SYNTHETIC);
    }

    function _fork(string memory json, string memory label) internal pure returns (bytes memory raw, bytes32 hash) {
        raw = vm.parseJsonBytes(json, string.concat(".forks.", label, ".raw"));
        hash = vm.parseJsonBytes32(json, string.concat(".forks.", label, ".hash"));
    }

    // --- header field helpers (little-endian fields of the 1487-byte header) ---

    function _time(bytes memory header) internal pure returns (uint256) {
        return _le32(header, 100);
    }

    function _bits(bytes memory header) internal pure returns (uint256) {
        return _le32(header, 104);
    }

    function _le32(bytes memory b, uint256 offset) internal pure returns (uint256 v) {
        v = uint256(uint8(b[offset])) | (uint256(uint8(b[offset + 1])) << 8) | (uint256(uint8(b[offset + 2])) << 16)
            | (uint256(uint8(b[offset + 3])) << 24);
    }

    function _setLe32(bytes memory b, uint256 offset, uint256 v) internal pure {
        b[offset] = bytes1(uint8(v));
        b[offset + 1] = bytes1(uint8(v >> 8));
        b[offset + 2] = bytes1(uint8(v >> 16));
        b[offset + 3] = bytes1(uint8(v >> 24));
    }

    function _bytes32At(bytes memory b, uint256 offset) internal pure returns (bytes32 v) {
        assembly {
            v := mload(add(add(b, 32), offset))
        }
    }

    function _copy(bytes memory b) internal pure returns (bytes memory c) {
        c = new bytes(b.length);
        for (uint256 i; i < b.length; ++i) {
            c[i] = b[i];
        }
    }

    function _toUint32(uint256[] memory a) internal pure returns (uint32[] memory r) {
        r = new uint32[](a.length);
        for (uint256 i; i < a.length; ++i) {
            r[i] = uint32(a[i]);
        }
    }
}
