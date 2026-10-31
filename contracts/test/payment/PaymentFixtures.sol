// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {RelayFixtures} from "../relay/RelayFixtures.sol";
import {ZcashHeaderRelay} from "../../src/relay/ZcashHeaderRelay.sol";
import {ZCASH_TESTNET} from "../../src/relay/IZcashHeaderRelay.sol";
import {IZcashHeaderRelayReader} from "../../src/interfaces/IZcashHeaderRelayReader.sol";
import {IRiscZeroVerifier} from "../../src/payment/IRiscZeroVerifier.sol";
import {ZcashPaymentVerifier} from "../../src/payment/ZcashPaymentVerifier.sol";
import {MockRiscZeroVerifier} from "./Mocks.sol";

/// @notice What the payment tests share: journals as the guest encodes them, a MOCK router, and relays with real
///         Testnet headers.
abstract contract PaymentFixtures is RelayFixtures {
    /// @dev RISC Zero's Groth16 v3.0.0 selector (RiscZeroVerifierRouter on Base Sepolia).
    bytes4 internal constant GROTH16_SELECTOR = 0x73c457ba;
    bytes32 internal constant IMAGE_ID = keccak256("MOCK payment-test image id");
    uint32 internal constant NU6_3_BRANCH_ID = 0x37A5_165B;
    uint32 internal constant NU7_BRANCH_ID = 0x7719_0AD9;
    string internal constant DEPOSIT_ANCHOR = "test/relay/fixtures/testnet_deposit_anchor.json";

    MockRiscZeroVerifier internal router;

    function _journal(bytes32 anchor, bytes32 ref, bytes32 receiver, uint64 zatoshi)
        internal
        pure
        returns (bytes memory)
    {
        return abi.encode(uint8(1), NU7_BRANCH_ID, anchor, ref, receiver, zatoshi);
    }

    function _seal(bytes memory journal) internal view returns (bytes memory) {
        return router.mockSeal(IMAGE_ID, sha256(journal));
    }

    function _testnetBranches() internal pure returns (uint32[] memory b) {
        b = new uint32[](1);
        b[0] = NU7_BRANCH_ID;
    }

    /// @dev A relay from `testnet_deposit_anchor.json`'s checkpoint fed with its 12 real headers: the first one,
    ///      4,488,560, has 11 on top. It is T, the anchor of the public transaction in crates/seam-payment's fixture.
    function _anchorRelay() internal returns (ZcashHeaderRelay relay, RealFixture memory fx) {
        fx = _loadReal(DEPOSIT_ANCHOR);
        relay = _deploy(fx);
        vm.warp(_time(fx.headers[fx.headers.length - 1]) + 60);
        relay.submitHeaders(fx.headers);
    }

    function _verifier(ZcashHeaderRelay relay) internal returns (ZcashPaymentVerifier) {
        router = new MockRiscZeroVerifier(GROTH16_SELECTOR);
        return new ZcashPaymentVerifier(
            IZcashHeaderRelayReader(address(relay)),
            IRiscZeroVerifier(address(router)),
            GROTH16_SELECTOR,
            IMAGE_ID,
            ZCASH_TESTNET,
            _testnetBranches()
        );
    }
}
