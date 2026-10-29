// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {Test} from "forge-std/Test.sol";
import {ZcashHeaderRelay} from "../../src/relay/ZcashHeaderRelay.sol";
import {DeployRelay} from "../../script/DeployRelay.s.sol";

/// @notice The checks of one Seam deployment record (deployments/<name>.relay.json) and its checkpoint file: they agree
///         with each other and with their network's configuration, and a relay deployed from them starts at that
///         checkpoint. With `BASE_SEPOLIA_RPC_URL` the recorded relay is also checked on Base Sepolia.
abstract contract SeamDeploymentTest is Test {
    uint256 internal constant BASE_SEPOLIA_CHAIN_ID = 84532;

    DeployRelay internal script;
    ZcashHeaderRelay.RelayParams internal params;
    DeployRelay.Checkpoint internal checkpoint;
    string internal record;

    /// @dev The deployment record, its checkpoint file, the configuration it names and that network's name.
    function _files()
        internal
        pure
        virtual
        returns (string memory recordPath, string memory checkpointPath, string memory config, string memory network);

    function setUp() public {
        (string memory recordPath, string memory checkpointPath, string memory config,) = _files();
        script = new DeployRelay();
        string memory display;
        (params,, display) = script.loadParams(config);
        checkpoint = script.loadCheckpoint(checkpointPath, display); // refuses a checkpoint of another network
        record = vm.readFile(recordPath);
    }

    function test_checkpointFileIsTheRecordsCheckpoint() public view {
        assertEq(checkpoint.height, vm.parseJsonUint(record, ".checkpointHeight"));
        assertEq(checkpoint.hash, vm.parseJsonBytes32(record, ".checkpointHash"));
        assertEq(checkpoint.times.length, 113);
        assertEq(checkpoint.bits.length, 113);

        // The context ends with the checkpoint, and the display hash is the checkpoint hash byte-reversed.
        (, string memory checkpointPath,,) = _files();
        string memory json = vm.readFile(checkpointPath);
        uint256[] memory heights = vm.parseJsonUintArray(json, ".context_heights");
        assertEq(heights.length, 113);
        assertEq(heights[0], checkpoint.height - 112);
        assertEq(heights[112], checkpoint.height);
        bytes32 display = vm.parseBytes32(string.concat("0x", vm.parseJsonString(json, ".checkpoint_hash_display")));
        assertEq(display, _reversed(checkpoint.hash));
    }

    function test_recordNamesItsConfiguration() public view {
        (,, string memory config, string memory network) = _files();
        assertEq(vm.parseJsonString(record, ".parameters"), config);
        assertEq(vm.parseJsonString(record, ".zcashNetwork"), network);
        assertEq(vm.parseJsonBytes32(record, ".networkId"), keccak256(bytes(network)));
        assertEq(vm.parseJsonUint(record, ".chainId"), BASE_SEPOLIA_CHAIN_ID);
        assertEq(vm.parseJsonUint(record, ".nu7ActivationHeight"), params.nu7ActivationHeight);
        assertEq(params.networkId, keccak256(bytes(network)));
    }

    function test_relayFromTheFilesStartsAtTheCheckpoint() public {
        ZcashHeaderRelay relay =
            new ZcashHeaderRelay(params, checkpoint.hash, checkpoint.height, checkpoint.times, checkpoint.bits);
        (bytes32 tip, uint256 height, uint256 work) = relay.bestChainTip();
        assertEq(tip, checkpoint.hash);
        assertEq(height, checkpoint.height);
        assertEq(work, 0);
        assertEq(relay.bestBlockHashAt(checkpoint.height), checkpoint.hash);
    }

    /// @notice The recorded relay on Base Sepolia (skipped without `BASE_SEPOLIA_RPC_URL`).
    function test_live_recordedRelayOnBaseSepolia() public {
        string memory rpc = vm.envOr("BASE_SEPOLIA_RPC_URL", string(""));
        if (bytes(rpc).length == 0) {
            vm.skip(true);
            return;
        }
        vm.createSelectFork(rpc);
        assertEq(block.chainid, BASE_SEPOLIA_CHAIN_ID);
        ZcashHeaderRelay relay = ZcashHeaderRelay(vm.parseJsonAddress(record, ".ZcashHeaderRelay"));
        assertGt(address(relay).code.length, 0, "no code at the recorded address");
        assertEq(relay.networkId(), params.networkId);
        assertEq(abi.encode(relay.params()), abi.encode(params));
        assertEq(relay.checkpointHash(), checkpoint.hash);
        assertEq(relay.checkpointHeight(), checkpoint.height);
        assertEq(relay.bestBlockHashAt(checkpoint.height), checkpoint.hash);
    }

    function _reversed(bytes32 h) internal pure returns (bytes32 r) {
        for (uint256 i; i < 32; ++i) {
            r |= bytes32(uint256(uint8(h[i])) << (8 * i));
        }
    }
}
