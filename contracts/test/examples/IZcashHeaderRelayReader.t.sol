// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {RelayFixtures} from "../relay/RelayFixtures.sol";
import {ZcashHeaderRelay} from "../../src/relay/ZcashHeaderRelay.sol";
import {IZcashHeaderRelay, ZCASH_MAINNET, ZCASH_TESTNET} from "../../src/relay/IZcashHeaderRelay.sol";
import {IZcashHeaderRelayReader, ZcashNetworks} from "../../src/interfaces/IZcashHeaderRelayReader.sol";

/// @notice IZcashHeaderRelayReader is the read side of IZcashHeaderRelay: the same selectors and return encodings, and
///         ZcashNetworks holds the same network ids.
contract IZcashHeaderRelayReaderTest is RelayFixtures {
    function test_selectorsAreTheRelayInterfaces() public pure {
        assertEq(IZcashHeaderRelayReader.networkId.selector, IZcashHeaderRelay.networkId.selector);
        assertEq(IZcashHeaderRelayReader.isInBestChain.selector, IZcashHeaderRelay.isInBestChain.selector);
        assertEq(IZcashHeaderRelayReader.confirmations.selector, IZcashHeaderRelay.confirmations.selector);
        assertEq(IZcashHeaderRelayReader.confirmationWork.selector, IZcashHeaderRelay.confirmationWork.selector);
        assertEq(IZcashHeaderRelayReader.headerOf.selector, IZcashHeaderRelay.headerOf.selector);
        assertEq(IZcashHeaderRelayReader.bestChainTip.selector, IZcashHeaderRelay.bestChainTip.selector);
        assertEq(IZcashHeaderRelayReader.bestBlockHashAt.selector, IZcashHeaderRelay.bestBlockHashAt.selector);
    }

    function test_networkIdsAreTheRelays() public pure {
        assertEq(ZcashNetworks.TESTNET, ZCASH_TESTNET);
        assertEq(ZcashNetworks.MAINNET, ZCASH_MAINNET);
    }

    /// @notice On a relay fed with the 32 real Testnet headers of testnet_recent.json, every view read through the
    ///         reader returns what the full interface returns, for the checkpoint, a middle header, the tip and an
    ///         unknown hash. The reader's types re-encode to the relay's raw return data.
    function test_readsAreTheRelayInterfaces() public {
        RealFixture memory fx = _loadReal(RECENT);
        ZcashHeaderRelay relay = _deploy(fx);
        uint256 n = fx.headers.length;
        vm.warp(_time(fx.headers[n - 1]) + 60);
        relay.submitHeaders(fx.headers);

        IZcashHeaderRelay full = IZcashHeaderRelay(address(relay));
        IZcashHeaderRelayReader reader = IZcashHeaderRelayReader(address(relay));
        assertEq(reader.networkId(), full.networkId());
        assertEq(reader.networkId(), ZcashNetworks.TESTNET);

        (bytes32 tip, uint256 tipHeight, uint256 tipWork) = reader.bestChainTip();
        (bytes32 fullTip, uint256 fullHeight, uint256 fullWork) = full.bestChainTip();
        assertEq(tip, fullTip);
        assertEq(tipHeight, fullHeight);
        assertEq(tipWork, fullWork);
        assertEq(tip, fx.hashes[n - 1]);
        assertEq(
            abi.encode(tip, tipHeight, tipWork),
            _raw(address(relay), abi.encodeCall(IZcashHeaderRelayReader.bestChainTip, ()))
        );

        bytes32[4] memory hashes = [fx.checkpointHash, fx.hashes[n / 2], fx.hashes[n - 1], keccak256("unknown")];
        for (uint256 i; i < hashes.length; ++i) {
            bytes32 h = hashes[i];
            assertEq(reader.isInBestChain(h), full.isInBestChain(h), "isInBestChain");
            assertEq(reader.confirmations(h), full.confirmations(h), "confirmations");
            assertEq(reader.confirmationWork(h), full.confirmationWork(h), "confirmationWork");
            assertEq(abi.encode(reader.headerOf(h)), abi.encode(full.headerOf(h)), "headerOf");
            assertEq(
                abi.encode(reader.headerOf(h)),
                _raw(address(relay), abi.encodeCall(IZcashHeaderRelayReader.headerOf, (h))),
                "raw headerOf"
            );
        }
        assertFalse(reader.headerOf(hashes[3]).known);
        assertEq(reader.confirmationWork(tip), 0);

        uint256[5] memory heights =
            [fx.checkpointHeight - 1, fx.checkpointHeight, fx.heights[n / 2], fx.heights[n - 1], fx.heights[n - 1] + 1];
        for (uint256 i; i < heights.length; ++i) {
            assertEq(reader.bestBlockHashAt(heights[i]), full.bestBlockHashAt(heights[i]), "bestBlockHashAt");
        }
        assertEq(reader.bestBlockHashAt(fx.checkpointHeight), fx.checkpointHash);
    }

    function _raw(address target, bytes memory data) internal view returns (bytes memory ret) {
        bool ok;
        (ok, ret) = target.staticcall(data);
        assertTrue(ok, "view call");
    }
}
