// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {Script, console} from "forge-std/Script.sol";
import {ZcashHeaderRelay} from "../src/relay/ZcashHeaderRelay.sol";

/// @title DeployRelay
/// @notice Deploys a ZcashHeaderRelay from a trusted checkpoint file in the format of test/relay/fixtures (built by
///         test/relay/tools/build_fixture.py: `checkpoint_hash`, `checkpoint_height`, and the times and nBits of the
///         113 blocks ending with the checkpoint). Headers are then submitted by anyone (scripts/relay-feed.sh).
/// @dev Environment: DEPLOYER_PRIVATE_KEY (never logged), RELAY_CHECKPOINT (path of the checkpoint file, readable
///      under fs_permissions), optional DEPLOYMENT_NAME (default base-sepolia): writes deployments/<name>.relay.json.
///        forge script script/DeployRelay.s.sol --rpc-url <rpc> --broadcast
contract DeployRelay is Script {
    function run() external returns (ZcashHeaderRelay relay) {
        // forge-lint: disable-next-line(unsafe-cheatcode)
        string memory json = vm.readFile(vm.envString("RELAY_CHECKPOINT"));
        bytes32 checkpointHash = vm.parseJsonBytes32(json, ".checkpoint_hash");
        uint256 checkpointHeight = vm.parseJsonUint(json, ".checkpoint_height");
        uint256[] memory times = vm.parseJsonUintArray(json, ".context_times");
        uint256[] memory bits = vm.parseJsonUintArray(json, ".context_bits");
        uint32[] memory contextTimes = new uint32[](times.length);
        uint32[] memory contextBits = new uint32[](bits.length);
        for (uint256 i; i < times.length; ++i) {
            contextTimes[i] = uint32(times[i]);
        }
        for (uint256 i; i < bits.length; ++i) {
            contextBits[i] = uint32(bits[i]);
        }

        vm.startBroadcast(vm.envUint("DEPLOYER_PRIVATE_KEY"));
        relay = new ZcashHeaderRelay(checkpointHash, checkpointHeight, contextTimes, contextBits);
        vm.stopBroadcast();

        console.log("ZcashHeaderRelay:", address(relay));
        console.log("checkpoint height:", checkpointHeight);
        string memory out = "relay";
        vm.serializeAddress(out, "ZcashHeaderRelay", address(relay));
        vm.serializeBytes32(out, "checkpointHash", checkpointHash);
        out = vm.serializeUint(out, "checkpointHeight", checkpointHeight);
        string memory name = vm.envOr("DEPLOYMENT_NAME", string("base-sepolia"));
        vm.writeJson(out, string.concat("deployments/", name, ".relay.json"));
    }
}
