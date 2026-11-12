// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {PaymentFixtures} from "./PaymentFixtures.sol";
import {MockRiscZeroVerifier} from "./Mocks.sol";
import {ZcashHeaderRelay} from "../../src/relay/ZcashHeaderRelay.sol";
import {ZCASH_MAINNET, ZCASH_TESTNET} from "../../src/relay/IZcashHeaderRelay.sol";
import {IZcashHeaderRelayReader} from "../../src/interfaces/IZcashHeaderRelayReader.sol";
import {IRiscZeroVerifier} from "../../src/payment/IRiscZeroVerifier.sol";
import {IZcashMaskedPaymentVerifier, ZcashMaskedPayment} from "../../src/payment/IZcashMaskedPaymentVerifier.sol";
import {ZcashMaskedPaymentVerifier} from "../../src/payment/ZcashMaskedPaymentVerifier.sol";
import {DeployMaskedPaymentVerifier} from "../../script/DeployMaskedPaymentVerifier.s.sol";

/// @notice ZcashMaskedPaymentVerifier (journal version 2) with a MOCK router on a relay fed with real Testnet headers.
contract ZcashMaskedPaymentVerifierTest is PaymentFixtures {
    ZcashHeaderRelay internal relay;
    RealFixture internal fx;
    ZcashMaskedPaymentVerifier internal verifier;
    bytes32 internal anchor;

    bytes32 internal constant MASKED_IMAGE_ID = keccak256("MOCK payment-masked-test image id");
    bytes32 internal constant REF = keccak256("paymentRef");
    bytes32 internal constant RECEIVER = keccak256("receiver commitment");
    uint64 internal constant M = 0xfedc_ba98_7654_3210;

    function setUp() public {
        (relay, fx) = _anchorRelay();
        router = new MockRiscZeroVerifier(GROTH16_SELECTOR);
        verifier = new ZcashMaskedPaymentVerifier(
            IZcashHeaderRelayReader(address(relay)),
            IRiscZeroVerifier(address(router)),
            GROTH16_SELECTOR,
            MASKED_IMAGE_ID,
            ZCASH_TESTNET,
            _testnetBranches()
        );
        anchor = fx.hashes[0];
    }

    function deployVerifier(
        IZcashHeaderRelayReader relay_,
        IRiscZeroVerifier router_,
        bytes4 selector_,
        bytes32 imageId_,
        bytes32 networkId_,
        uint32[] memory branchIds
    ) external returns (ZcashMaskedPaymentVerifier) {
        return new ZcashMaskedPaymentVerifier(relay_, router_, selector_, imageId_, networkId_, branchIds);
    }

    function _masked(bytes32 anchor_, bytes32 ref, bytes32 receiver, uint64 m) internal pure returns (bytes memory) {
        return abi.encode(uint8(2), NU7_BRANCH_ID, anchor_, ref, receiver, m);
    }

    function _mseal(bytes memory journal) internal view returns (bytes memory) {
        return router.mockSeal(MASKED_IMAGE_ID, sha256(journal));
    }

    /// @notice R as the guest derives it (crates/seam-payment/src/masked.rs `amount_mask`).
    function _mask(bytes32 salt, bytes32 ref) internal pure returns (uint64) {
        return uint64(bytes8(keccak256(abi.encodePacked("seam/amount-mask/v1", salt, ref))));
    }

    /// @notice The mask matches the vectors the Rust statement and the TypeScript tooling check (computed with ethers).
    function test_maskMatchesTheStatementsVectors() public pure {
        assertEq(_mask(bytes32(0), bytes32(0)), 0x0cd04af72b7b9602);
        assertEq(_mask(bytes32(type(uint256).max), bytes32(0)), 0xdfa08b7c9f656d57);
    }

    function test_journalLayoutIsTheStatements() public pure {
        bytes memory j = abi.encode(uint8(2), NU7_BRANCH_ID, bytes32(uint256(1)), REF, RECEIVER, uint64(0x010203040506));
        assertEq(j.length, 192);
        assertEq(uint8(j[31]), 2);
        assertEq(uint8(j[60]), 0x77);
        assertEq(uint8(j[63]), 0xd9);
        assertEq(uint8(j[186]), 1);
        assertEq(uint8(j[191]), 6);
    }

    function test_verify_returnsThePaymentAndTheWorkOnItsAnchor() public view {
        bytes memory j = _masked(anchor, REF, RECEIVER, M);
        (ZcashMaskedPayment memory p, uint256 work) = verifier.verify(j, _mseal(j));
        assertEq(p.branchId, NU7_BRANCH_ID);
        assertEq(p.anchor, anchor);
        assertEq(p.paymentRef, REF);
        assertEq(p.receiverCommitment, RECEIVER);
        assertEq(p.maskedAmount, M);
        assertEq(work, relay.confirmationWork(anchor));
        assertGt(work, 0);
        assertEq(verifier.imageId(), MASKED_IMAGE_ID);
        assertEq(verifier.networkId(), ZCASH_TESTNET);
    }

    /// @notice A version 1 journal (an amount in clear) is refused, and so is any other version.
    function test_verify_refusesOtherVersionsAndMalformedJournals() public {
        bytes memory j = _masked(anchor, REF, RECEIVER, M);
        bytes memory seal = _mseal(j);
        vm.expectRevert(abi.encodeWithSelector(IZcashMaskedPaymentVerifier.InvalidJournalLength.selector, 191));
        verifier.verify(new bytes(191), seal);
        for (uint256 v; v < 4; ++v) {
            if (v == 2) continue;
            bytes memory other = abi.encode(v, NU7_BRANCH_ID, anchor, REF, RECEIVER, M);
            bytes memory otherSeal = _mseal(other);
            vm.expectRevert(abi.encodeWithSelector(IZcashMaskedPaymentVerifier.UnsupportedJournalVersion.selector, v));
            verifier.verify(other, otherSeal);
        }
        bytes memory nu63 = abi.encode(uint8(2), NU6_3_BRANCH_ID, anchor, REF, RECEIVER, M);
        seal = _mseal(nu63);
        vm.expectRevert(abi.encodeWithSelector(IZcashMaskedPaymentVerifier.BranchNotAllowed.selector, NU6_3_BRANCH_ID));
        verifier.verify(nu63, seal);
        bytes memory huge = abi.encode(uint8(2), NU7_BRANCH_ID, anchor, REF, RECEIVER, uint256(type(uint64).max) + 1);
        seal = _mseal(huge);
        vm.expectRevert(IZcashMaskedPaymentVerifier.MalformedJournal.selector);
        verifier.verify(huge, seal);
    }

    /// @notice A seal of the version 1 guest, or of another journal or selector, doesn't verify.
    function test_verify_refusesSealsOfAnotherGuestSelectorOrJournal() public {
        bytes memory j = _masked(anchor, REF, RECEIVER, M);
        bytes memory v1Guest = router.mockSeal(IMAGE_ID, sha256(j));
        vm.expectRevert(MockRiscZeroVerifier.MockSealRejected.selector);
        verifier.verify(j, v1Guest);
        bytes memory anotherJournal = _mseal(_masked(anchor, REF, RECEIVER, M + 1));
        vm.expectRevert(MockRiscZeroVerifier.MockSealRejected.selector);
        verifier.verify(j, anotherJournal);
        bytes memory other = abi.encodePacked(bytes4(0x12345678), MASKED_IMAGE_ID, sha256(j));
        vm.expectRevert(
            abi.encodeWithSelector(IZcashMaskedPaymentVerifier.SelectorMismatch.selector, bytes4(0x12345678))
        );
        verifier.verify(j, other);
    }

    function test_verify_refusesAnAnchorOffTheBestChain() public {
        bytes32 unknown = keccak256("not a header");
        bytes memory j = _masked(unknown, REF, RECEIVER, M);
        bytes memory seal = _mseal(j);
        vm.expectRevert(abi.encodeWithSelector(IZcashMaskedPaymentVerifier.AnchorNotInBestChain.selector, unknown));
        verifier.verify(j, seal);
    }

    function test_constructor_refusesInconsistentInput() public {
        IZcashHeaderRelayReader r = IZcashHeaderRelayReader(address(relay));
        IRiscZeroVerifier v = IRiscZeroVerifier(address(router));
        uint32[] memory b = _testnetBranches();
        vm.expectRevert(ZcashMaskedPaymentVerifier.ZeroAddress.selector);
        this.deployVerifier(IZcashHeaderRelayReader(address(0)), v, GROTH16_SELECTOR, MASKED_IMAGE_ID, ZCASH_TESTNET, b);
        vm.expectRevert(ZcashMaskedPaymentVerifier.ZeroSelectorOrImageId.selector);
        this.deployVerifier(r, v, GROTH16_SELECTOR, bytes32(0), ZCASH_TESTNET, b);
        vm.expectRevert(
            abi.encodeWithSelector(ZcashMaskedPaymentVerifier.WrongNetwork.selector, ZCASH_TESTNET, ZCASH_MAINNET)
        );
        this.deployVerifier(r, v, GROTH16_SELECTOR, MASKED_IMAGE_ID, ZCASH_MAINNET, b);
        vm.expectRevert(ZcashMaskedPaymentVerifier.InvalidBranchIds.selector);
        this.deployVerifier(r, v, GROTH16_SELECTOR, MASKED_IMAGE_ID, ZCASH_TESTNET, new uint32[](0));
    }

    /// @notice The deploy script names each deployment's record, and none overwrites another's.
    function test_deployScript_recordPath() public {
        DeployMaskedPaymentVerifier script = new DeployMaskedPaymentVerifier();
        assertEq(script.recordPath("anvil"), "deployments/anvil.masked-payment-verifier.json");
        vm.expectRevert(bytes("DeployMaskedPaymentVerifier: set DEPLOYMENT_NAME"));
        script.recordPath("");
    }
}
