// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {SeamDeploymentTest} from "./SeamDeployment.sol";

/// @notice The Seam relay on Base Sepolia that follows Zcash Testnet (deployments/seam-base-sepolia.relay.json).
contract SeamBaseSepoliaTest is SeamDeploymentTest {
    function _files() internal pure override returns (string memory, string memory, string memory, string memory) {
        return (
            "deployments/seam-base-sepolia.relay.json",
            "deployments/seam-base-sepolia.relay-checkpoint.json",
            "config/relay-testnet.json",
            "zcash-testnet"
        );
    }
}
