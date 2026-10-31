// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {Script} from "forge-std/Script.sol";
import {ZCASH_MAINNET, ZCASH_TESTNET} from "../src/relay/IZcashHeaderRelay.sol";
import {IZcashHeaderRelayReader} from "../src/interfaces/IZcashHeaderRelayReader.sol";
import {IRiscZeroVerifier} from "../src/payment/IRiscZeroVerifier.sol";
import {ZcashPaymentVerifier} from "../src/payment/ZcashPaymentVerifier.sol";

/// @title DeployPaymentVerifier
/// @notice Deploys a ZcashPaymentVerifier for a Seam relay and writes deployments/<name>.payment-verifier.json.
/// @dev Environment: DEPLOYER_PRIVATE_KEY (never logged); PAYMENT_RELAY (the relay, whose `networkId()` names the
///      network); RISC0_ROUTER and GROTH16_SELECTOR (RISC Zero's verifier router on this chain and the selector of its
///      Groth16 verifier); PAYMENT_IMAGE_ID (that network's guest in zkvm/pinned-image-ids.txt); DEPLOYMENT_NAME,
///      required. The branch ids it allows follow the network: NU7's on Testnet, NU6.3's and NU7's on Mainnet.
///        PAYMENT_RELAY=<relay> RISC0_ROUTER=<router> GROTH16_SELECTOR=<selector> PAYMENT_IMAGE_ID=<image id> \
///          DEPLOYMENT_NAME=<name> forge script script/DeployPaymentVerifier.s.sol --rpc-url <rpc> --broadcast
contract DeployPaymentVerifier is Script {
    uint32 internal constant NU6_3_BRANCH_ID = 0x37A5_165B;
    uint32 internal constant NU7_BRANCH_ID = 0x7719_0AD9;

    function run() external returns (ZcashPaymentVerifier verifier) {
        IZcashHeaderRelayReader relay = IZcashHeaderRelayReader(vm.envAddress("PAYMENT_RELAY"));
        address router = vm.envAddress("RISC0_ROUTER");
        bytes4 selector = bytes4(vm.envBytes("GROTH16_SELECTOR"));
        bytes32 imageId = vm.envBytes32("PAYMENT_IMAGE_ID");
        string memory name = vm.envString("DEPLOYMENT_NAME");
        require(bytes(name).length != 0, "DeployPaymentVerifier: set DEPLOYMENT_NAME");
        bytes32 networkId = relay.networkId();
        uint32[] memory branches = branchIds(networkId);

        vm.startBroadcast(vm.envUint("DEPLOYER_PRIVATE_KEY"));
        verifier = new ZcashPaymentVerifier(relay, IRiscZeroVerifier(router), selector, imageId, networkId, branches);
        vm.stopBroadcast();

        uint256[] memory recorded = new uint256[](branches.length);
        for (uint256 i; i < branches.length; ++i) {
            recorded[i] = branches[i];
        }
        string memory out = "verifier";
        vm.serializeAddress(out, "ZcashPaymentVerifier", address(verifier));
        vm.serializeAddress(out, "relay", address(relay));
        vm.serializeAddress(out, "riscZeroVerifier", router);
        vm.serializeBytes(out, "selector", abi.encodePacked(selector));
        vm.serializeBytes32(out, "imageId", imageId);
        vm.serializeBytes32(out, "networkId", networkId);
        vm.serializeUint(out, "branchIds", recorded);
        out = vm.serializeUint(out, "chainId", block.chainid);
        vm.writeJson(out, string.concat("deployments/", name, ".payment-verifier.json"));
    }

    /// @notice The branch ids a verifier of `networkId`'s guest allows: the epochs that guest proves.
    function branchIds(bytes32 networkId) public pure returns (uint32[] memory ids) {
        if (networkId == ZCASH_TESTNET) {
            ids = new uint32[](1);
            ids[0] = NU7_BRANCH_ID;
        } else if (networkId == ZCASH_MAINNET) {
            ids = new uint32[](2);
            ids[0] = NU6_3_BRANCH_ID;
            ids[1] = NU7_BRANCH_ID;
        } else {
            revert("DeployPaymentVerifier: not a Zcash network");
        }
    }
}
