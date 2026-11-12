// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {RelayFixtures} from "./RelayFixtures.sol";
import {ZcashHeaderRelay} from "../../src/relay/ZcashHeaderRelay.sol";

/// @notice The relay must deploy in one transaction on a chain that prices code like Ethereum Sepolia since
///         Glamsterdam (October 2026): about 1,560 gas per byte of deployed code, and at most 2^24 gas per transaction
///         (EIP-7825). Measured on Sepolia on 12 November 2026 with `eth_estimateGas`: the relay's 9,862 bytes of code
///         alone cost 15,571,373 gas, and its whole deployment from a real Testnet checkpoint 19,586,183 gas: the
///         constructor's storage (the parameters, the checkpoint and its 113 context entries) adds about 4.0M. Code of
///         at most 7,900 bytes leaves room for that storage under 2^24.
contract ZcashHeaderRelaySizeTest is RelayFixtures {
    uint256 internal constant SEPOLIA_DEPLOYABLE_CODE_BYTES = 7_900;

    function test_relayCodeFitsOneSepoliaTransaction() public {
        ZcashHeaderRelay relay = _deploy(_loadReal("test/relay/fixtures/testnet_recent.json"));
        assertLe(address(relay).code.length, SEPOLIA_DEPLOYABLE_CODE_BYTES, "the relay's code is too large for Sepolia");
    }
}
