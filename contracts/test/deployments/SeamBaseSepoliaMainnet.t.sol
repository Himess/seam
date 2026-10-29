// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {SeamDeploymentTest} from "./SeamDeployment.sol";

/// @notice The Seam relay on Base Sepolia that follows Zcash Mainnet
///         (deployments/seam-base-sepolia-mainnet.relay.json).
contract SeamBaseSepoliaMainnetTest is SeamDeploymentTest {
    function _files() internal pure override returns (string memory, string memory, string memory, string memory) {
        return (
            "deployments/seam-base-sepolia-mainnet.relay.json",
            "deployments/seam-base-sepolia-mainnet.relay-checkpoint.json",
            "config/relay-mainnet.json",
            "zcash-mainnet"
        );
    }
}
