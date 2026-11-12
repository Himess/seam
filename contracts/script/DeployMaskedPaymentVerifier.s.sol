// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {Script, console} from "forge-std/Script.sol";
import {VmSafe} from "forge-std/Vm.sol";
import {IZcashHeaderRelayReader} from "../src/interfaces/IZcashHeaderRelayReader.sol";
import {IRiscZeroVerifier} from "../src/payment/IRiscZeroVerifier.sol";
import {ZcashMaskedPaymentVerifier} from "../src/payment/ZcashMaskedPaymentVerifier.sol";
import {DeployPaymentVerifier} from "./DeployPaymentVerifier.s.sol";

/// @title DeployMaskedPaymentVerifier
/// @notice Deploys a ZcashMaskedPaymentVerifier (amount-hidden proofs, journal version 2) for a Seam relay and, when it
///         broadcasts, writes its record, deployments/<name>.masked-payment-verifier.json.
/// @dev Environment as DeployPaymentVerifier's, with MASKED_IMAGE_ID (that network's `payment-masked-*` guest in
///      zkvm/pinned-image-ids.txt) in place of PAYMENT_IMAGE_ID:
///        PAYMENT_RELAY=<relay> RISC0_ROUTER=<router> GROTH16_SELECTOR=<selector> MASKED_IMAGE_ID=<image id> \
///          DEPLOYMENT_NAME=<name> forge script script/DeployMaskedPaymentVerifier.s.sol --rpc-url <rpc> --broadcast
contract DeployMaskedPaymentVerifier is Script {
    function run() external returns (ZcashMaskedPaymentVerifier verifier) {
        IZcashHeaderRelayReader relay = IZcashHeaderRelayReader(vm.envAddress("PAYMENT_RELAY"));
        address router = vm.envAddress("RISC0_ROUTER");
        bytes4 selector = bytes4(vm.envBytes("GROTH16_SELECTOR"));
        bytes32 imageId = vm.envBytes32("MASKED_IMAGE_ID");
        string memory record = recordPath(vm.envOr("DEPLOYMENT_NAME", string("")));
        bytes32 networkId = relay.networkId();
        // The same epochs as version 1's guest of that network.
        uint32[] memory branches = new DeployPaymentVerifier().branchIds(networkId);

        vm.startBroadcast(vm.envUint("DEPLOYER_PRIVATE_KEY"));
        verifier =
            new ZcashMaskedPaymentVerifier(relay, IRiscZeroVerifier(router), selector, imageId, networkId, branches);
        vm.stopBroadcast();

        uint256[] memory recorded = new uint256[](branches.length);
        for (uint256 i; i < branches.length; ++i) {
            recorded[i] = branches[i];
        }
        string memory out = "verifier";
        vm.serializeAddress(out, "ZcashMaskedPaymentVerifier", address(verifier));
        vm.serializeAddress(out, "relay", address(relay));
        vm.serializeAddress(out, "riscZeroVerifier", router);
        vm.serializeBytes(out, "selector", abi.encodePacked(selector));
        vm.serializeBytes32(out, "imageId", imageId);
        vm.serializeBytes32(out, "networkId", networkId);
        vm.serializeUint(out, "branchIds", recorded);
        vm.serializeUint(out, "journalVersion", 2);
        out = vm.serializeUint(out, "chainId", block.chainid);
        if (vm.isContext(VmSafe.ForgeContext.ScriptBroadcast)) {
            vm.writeJson(out, record);
            console.log("record:", record);
        } else {
            console.log("not broadcast: no record written");
        }
    }

    /// @notice The deployment's record, deployments/<name>.masked-payment-verifier.json, for `name` (DEPLOYMENT_NAME).
    ///         A name whose record exists is refused, so that no deployment overwrites another's record.
    function recordPath(string memory name) public view returns (string memory path) {
        require(bytes(name).length != 0, "DeployMaskedPaymentVerifier: set DEPLOYMENT_NAME");
        path = string.concat("deployments/", name, ".masked-payment-verifier.json");
        require(
            !vm.exists(path),
            string.concat("DeployMaskedPaymentVerifier: ", path, " exists: choose another DEPLOYMENT_NAME")
        );
    }
}
