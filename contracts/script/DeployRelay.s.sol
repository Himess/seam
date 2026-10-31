// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {Script, console} from "forge-std/Script.sol";
import {ZCASH_TESTNET} from "../src/relay/IZcashHeaderRelay.sol";
import {ZcashHeaderRelay} from "../src/relay/ZcashHeaderRelay.sol";

/// @title DeployRelay
/// @notice Deploys a ZcashHeaderRelay for one Zcash network from two files: the network's consensus parameters
///         (config/relay-<network>.json) and a trusted checkpoint in the format of test/relay/fixtures (built by
///         test/relay/tools/build_fixture.py: `checkpoint_hash`, `checkpoint_height`, the times and nBits of the 113
///         blocks ending with the checkpoint, and `network` when known). Headers are then submitted by anyone
///         (scripts/relay-feed.sh, the bots' relayer).
/// @dev Environment: DEPLOYER_PRIVATE_KEY (never logged); RELAY_CHECKPOINT (path of the checkpoint file, readable
///      under fs_permissions); optional RELAY_NETWORK (`testnet`, the default, or `mainnet`: config/relay-<network>.json)
///      or RELAY_CONFIG (the path of another parameter file); DEPLOYMENT_NAME: writes deployments/<name>.relay.json,
///      required, so that no deployment overwrites another's record. A checkpoint file that names its network must name
///      the configuration's.
///      The relay does not depend on the Base chain it runs on, so no chain id is checked; the output records it.
///        RELAY_CHECKPOINT=<testnet checkpoint> DEPLOYMENT_NAME=my-testnet-relay \
///          forge script script/DeployRelay.s.sol --rpc-url <rpc> --broadcast
///        RELAY_NETWORK=mainnet RELAY_CHECKPOINT=<mainnet checkpoint> DEPLOYMENT_NAME=base-sepolia-mainnet-shadow \
///          forge script script/DeployRelay.s.sol --rpc-url <rpc> --broadcast
contract DeployRelay is Script {
    /// @notice A trusted checkpoint: the relay's constructor input besides the parameters.
    struct Checkpoint {
        bytes32 hash;
        uint256 height;
        uint32[] times;
        uint32[] bits;
    }

    function run() external returns (ZcashHeaderRelay relay) {
        string memory network = vm.envOr("RELAY_NETWORK", string("testnet"));
        string memory configPath = vm.envOr("RELAY_CONFIG", string.concat("config/relay-", network, ".json"));
        (ZcashHeaderRelay.RelayParams memory p, string memory zcashNetwork, string memory displayName) =
            loadParams(configPath);
        checkNu7ActivationHeight(p.nu7ActivationHeight);
        Checkpoint memory cp = loadCheckpoint(vm.envString("RELAY_CHECKPOINT"), displayName);
        string memory name = deploymentName(p.networkId, vm.envOr("DEPLOYMENT_NAME", string("")));

        vm.startBroadcast(vm.envUint("DEPLOYER_PRIVATE_KEY"));
        relay = new ZcashHeaderRelay(p, cp.hash, cp.height, cp.times, cp.bits);
        vm.stopBroadcast();

        console.log("ZcashHeaderRelay:", address(relay));
        console.log("Zcash network:", zcashNetwork);
        console.log("parameters:", configPath);
        console.log("checkpoint height:", cp.height);
        console.log("NU7 activation height:", p.nu7ActivationHeight);
        if (p.nu7ActivationHeight == type(uint32).max) {
            console.log("WARNING: no NU7 activation height: this relay refuses the network's first NU7 block");
        }
        string memory out = "relay";
        vm.serializeAddress(out, "ZcashHeaderRelay", address(relay));
        vm.serializeBytes32(out, "checkpointHash", cp.hash);
        vm.serializeUint(out, "checkpointHeight", cp.height);
        vm.serializeString(out, "zcashNetwork", zcashNetwork);
        vm.serializeBytes32(out, "networkId", p.networkId);
        vm.serializeString(out, "parameters", configPath);
        vm.serializeUint(out, "nu7ActivationHeight", p.nu7ActivationHeight);
        out = vm.serializeUint(out, "chainId", block.chainid);
        vm.writeJson(out, string.concat("deployments/", name, ".relay.json"));
    }

    /// @notice The name of the deployment record (deployments/<name>.relay.json): `requested` (DEPLOYMENT_NAME), which
    ///         every deployment sets, so that none overwrites another's record (`base-sepolia` is Zecured's live run).
    function deploymentName(bytes32, string memory requested) public pure returns (string memory) {
        require(bytes(requested).length != 0, "DeployRelay: set DEPLOYMENT_NAME");
        return requested;
    }

    /// @notice ZIP 259 requires the NU7 activation height to be a multiple of 3 (ZIP 214 revision 3): catches a
    ///         mistyped height before it is fixed in an immutable relay. `NEVER` (type(uint32).max) passes.
    function checkNu7ActivationHeight(uint32 height) public pure {
        require(
            height == type(uint32).max || height % 3 == 0,
            "DeployRelay: the NU7 activation height is not a multiple of 3 (ZIP 259)"
        );
    }

    /// @notice Reads a parameter file (config/relay-<network>.json).
    /// @return p The relay parameters; `networkId` is keccak256 of the file's `zcashNetwork`.
    /// @return zcashNetwork The network's name (`zcash-testnet`, `zcash-mainnet`).
    /// @return displayName The name checkpoint files use for that network (`Zcash Testnet`, `Zcash Mainnet`).
    function loadParams(string memory path)
        public
        view
        returns (ZcashHeaderRelay.RelayParams memory p, string memory zcashNetwork, string memory displayName)
    {
        // forge-lint: disable-next-line(unsafe-cheatcode)
        string memory json = vm.readFile(path);
        zcashNetwork = vm.parseJsonString(json, ".zcashNetwork");
        displayName = vm.parseJsonString(json, ".displayName");
        p.networkId = keccak256(bytes(zcashNetwork));
        p.powLimit = vm.parseJsonUint(json, ".powLimit");
        p.powLimitBits = _uint32(vm.parseJsonUint(json, ".powLimitBits"));
        p.minDifficultyBlocks = vm.parseJsonBool(json, ".minDifficultyBlocks");
        p.minDifficultyStartHeight = _uint32(vm.parseJsonUint(json, ".minDifficultyStartHeight"));
        p.maxTimeStartHeight = _uint32(vm.parseJsonUint(json, ".maxTimeStartHeight"));
        p.nu7ActivationHeight = _uint32(vm.parseJsonUint(json, ".nu7ActivationHeight"));
        p.preNu7TargetSpacing = _uint32(vm.parseJsonUint(json, ".preNu7TargetSpacing"));
        p.preNu7AveragingWindow = _uint32(vm.parseJsonUint(json, ".preNu7AveragingWindow"));
        p.postNu7TargetSpacing = _uint32(vm.parseJsonUint(json, ".postNu7TargetSpacing"));
        p.postNu7AveragingWindow = _uint32(vm.parseJsonUint(json, ".postNu7AveragingWindow"));
        p.minCheckpointHeight = _uint32(vm.parseJsonUint(json, ".minCheckpointHeight"));
    }

    /// @notice Reads a checkpoint file. If it names its network (`network`), that must be `displayName`.
    function loadCheckpoint(string memory path, string memory displayName) public view returns (Checkpoint memory cp) {
        // forge-lint: disable-next-line(unsafe-cheatcode)
        string memory json = vm.readFile(path);
        if (vm.keyExistsJson(json, ".network")) {
            string memory named = vm.parseJsonString(json, ".network");
            require(
                keccak256(bytes(named)) == keccak256(bytes(displayName)),
                string.concat("DeployRelay: the checkpoint is on ", named, ", the parameters are for ", displayName)
            );
        }
        cp.hash = vm.parseJsonBytes32(json, ".checkpoint_hash");
        cp.height = vm.parseJsonUint(json, ".checkpoint_height");
        uint256[] memory times = vm.parseJsonUintArray(json, ".context_times");
        uint256[] memory bits = vm.parseJsonUintArray(json, ".context_bits");
        cp.times = new uint32[](times.length);
        cp.bits = new uint32[](bits.length);
        for (uint256 i; i < times.length; ++i) {
            cp.times[i] = _uint32(times[i]);
        }
        for (uint256 i; i < bits.length; ++i) {
            cp.bits[i] = _uint32(bits[i]);
        }
    }

    function _uint32(uint256 v) private pure returns (uint32) {
        require(v <= type(uint32).max, "DeployRelay: value does not fit in 32 bits");
        return uint32(v);
    }
}
