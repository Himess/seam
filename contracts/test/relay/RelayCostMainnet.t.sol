// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {console} from "forge-std/console.sol";
import {RelayFixtures} from "./RelayFixtures.sol";
import {ZcashHeaderRelay} from "../../src/relay/ZcashHeaderRelay.sol";

/// @notice The OP Stack's L1 fee oracle, a Base predeploy.
interface IGasPriceOracle {
    function getL1Fee(bytes memory data) external view returns (uint256);
}

/// @notice What feeding the Mainnet relay costs per header, by batch size: the relay's execution gas and the
///         transaction's calldata (docs/MAINNET.md, "Relay cost"). With `BASE_MAINNET_RPC_URL` it also prices each
///         batch on a Base mainnet fork: the L2 execution fee at the fork's base fee and the L1 data fee from the
///         GasPriceOracle predeploy (measured on 2026-10-17: the L2 execution fee is almost all of it).
contract RelayCostMainnetTest is RelayFixtures {
    address internal constant GAS_PRICE_ORACLE = 0x420000000000000000000000000000000000000F;
    uint256 internal constant TX_BASE_GAS = 21_000;

    struct Cost {
        uint256 size;
        uint256 executionGas;
        uint256 calldataBytes;
        uint256 calldataGas;
    }

    function _batch(RealFixture memory fx, uint256 size) internal pure returns (bytes[] memory batch) {
        batch = new bytes[](size);
        for (uint256 i; i < size; ++i) {
            batch[i] = fx.headers[i];
        }
    }

    /// Execution gas of one `submitHeaders` call of the first `size` headers on a fresh relay, and its calldata.
    function _measure(RealFixture memory fx, uint256 size) internal returns (Cost memory c) {
        ZcashHeaderRelay relay = _deploy(fx);
        bytes[] memory batch = _batch(fx, size);
        _warpTo(fx.headers[size - 1]);
        uint256 before = gasleft();
        relay.submitHeaders(batch);
        c.executionGas = before - gasleft();
        bytes memory data = abi.encodeCall(ZcashHeaderRelay.submitHeaders, (batch));
        c.size = size;
        c.calldataBytes = data.length;
        for (uint256 i; i < data.length; ++i) {
            c.calldataGas += data[i] == 0 ? 4 : 16;
        }
    }

    function test_mainnet_relayCost_byBatchSize() public {
        RealFixture memory fx = _loadReal(MAINNET_RECENT);
        uint256[4] memory sizes = [uint256(1), 4, 8, 16];
        uint256 single;
        for (uint256 k; k < sizes.length; ++k) {
            Cost memory c = _measure(fx, sizes[k]);
            uint256 total = TX_BASE_GAS + c.calldataGas + c.executionGas;
            uint256 perHeader = total / c.size;
            if (k == 0) single = perHeader;
            console.log("batch", c.size, "headers: total L2 gas", total);
            console.log("  per header: gas", perHeader, "calldata bytes", c.calldataBytes / c.size);
            // Batching only saves the per-transaction parts: never more per header than one by one.
            assertLe(perHeader, single);
        }
    }

    /// The same batches priced on Base mainnet (skipped without `BASE_MAINNET_RPC_URL`).
    function test_mainnet_relayCost_onBaseMainnet() public {
        string memory rpc = vm.envOr("BASE_MAINNET_RPC_URL", string(""));
        if (bytes(rpc).length == 0) {
            vm.skip(true);
            return;
        }
        uint256 forkBlock = vm.envOr("ZECURED_FORK_BLOCK", uint256(0));
        if (forkBlock == 0) vm.createSelectFork(rpc);
        else vm.createSelectFork(rpc, forkBlock);
        RealFixture memory fx = _loadReal(MAINNET_RECENT);
        uint256 baseFee = block.basefee;
        console.log("Base mainnet block", block.number, "base fee (wei)", baseFee);
        uint256[4] memory sizes = [uint256(1), 4, 8, 16];
        for (uint256 k; k < sizes.length; ++k) {
            Cost memory c = _measure(fx, sizes[k]);
            bytes memory data = abi.encodeCall(ZcashHeaderRelay.submitHeaders, (_batch(fx, sizes[k])));
            uint256 l1Fee = IGasPriceOracle(GAS_PRICE_ORACLE).getL1Fee(data);
            uint256 l2Fee = (TX_BASE_GAS + c.calldataGas + c.executionGas) * baseFee;
            console.log("batch", c.size, "headers: L1 fee per header (wei)", l1Fee / c.size);
            console.log("  L2 execution fee per header (wei)", l2Fee / c.size);
        }
    }
}
