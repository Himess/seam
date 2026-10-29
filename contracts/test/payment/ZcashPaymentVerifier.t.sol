// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {PaymentFixtures} from "./PaymentFixtures.sol";
import {MockRiscZeroVerifier} from "./Mocks.sol";
import {ZcashHeaderRelay} from "../../src/relay/ZcashHeaderRelay.sol";
import {ZCASH_MAINNET, ZCASH_TESTNET} from "../../src/relay/IZcashHeaderRelay.sol";
import {IZcashHeaderRelayReader} from "../../src/interfaces/IZcashHeaderRelayReader.sol";
import {IRiscZeroVerifier} from "../../src/payment/IRiscZeroVerifier.sol";
import {IZcashPaymentVerifier, ZcashPayment} from "../../src/payment/IZcashPaymentVerifier.sol";
import {ZcashPaymentVerifier} from "../../src/payment/ZcashPaymentVerifier.sol";

/// @notice ZcashPaymentVerifier with a MOCK router on a relay fed with real Testnet headers. The fork test with a real
///         seal through RISC Zero's router is test/payment/PaymentFork.t.sol.
contract ZcashPaymentVerifierTest is PaymentFixtures {
    ZcashHeaderRelay internal relay;
    RealFixture internal fx;
    ZcashPaymentVerifier internal verifier;
    bytes32 internal anchor;

    bytes32 internal constant REF = keccak256("paymentRef");
    bytes32 internal constant RECEIVER = keccak256("receiver commitment");
    uint64 internal constant ZAT = 2_500_000;

    function setUp() public {
        (relay, fx) = _anchorRelay();
        verifier = _verifier(relay);
        anchor = fx.hashes[0];
    }

    function deployVerifier(
        IZcashHeaderRelayReader relay_,
        IRiscZeroVerifier router_,
        bytes4 selector_,
        bytes32 imageId_,
        bytes32 networkId_,
        uint32[] memory branchIds
    ) external returns (ZcashPaymentVerifier) {
        return new ZcashPaymentVerifier(relay_, router_, selector_, imageId_, networkId_, branchIds);
    }

    /// @notice The guest's journal layout (crates/seam-payment `journal_abi_layout`): six words, the small ones
    ///         right-aligned.
    function test_journalLayoutIsTheStatements() public pure {
        bytes memory j = abi.encode(uint8(1), NU7_BRANCH_ID, bytes32(uint256(1)), REF, RECEIVER, uint64(0x010203040506));
        assertEq(j.length, 192);
        assertEq(uint8(j[31]), 1);
        assertEq(uint8(j[60]), 0x77);
        assertEq(uint8(j[63]), 0xd9);
        assertEq(uint8(j[186]), 1);
        assertEq(uint8(j[191]), 6);
    }

    function test_verify_returnsThePaymentAndTheWorkOnItsAnchor() public view {
        bytes memory j = _journal(anchor, REF, RECEIVER, ZAT);
        (ZcashPayment memory p, uint256 work) = verifier.verify(j, _seal(j));
        assertEq(p.branchId, NU7_BRANCH_ID);
        assertEq(p.anchor, anchor);
        assertEq(p.paymentRef, REF);
        assertEq(p.receiverCommitment, RECEIVER);
        assertEq(p.zatoshi, ZAT);
        assertEq(work, relay.confirmationWork(anchor));
        assertGt(work, 0, "11 headers on top of the anchor");
        assertEq(verifier.relay(), address(relay));
        assertEq(verifier.riscZeroVerifier(), address(router));
        assertEq(verifier.selector(), GROTH16_SELECTOR);
        assertEq(verifier.imageId(), IMAGE_ID);
        assertEq(verifier.networkId(), ZCASH_TESTNET);
    }

    function test_verify_refusesMalformedJournals() public {
        bytes memory j = _journal(anchor, REF, RECEIVER, ZAT);
        bytes memory seal = _seal(j);
        bytes memory short = new bytes(191);
        vm.expectRevert(abi.encodeWithSelector(IZcashPaymentVerifier.InvalidJournalLength.selector, 191));
        verifier.verify(short, seal);
        vm.expectRevert(abi.encodeWithSelector(IZcashPaymentVerifier.InvalidJournalLength.selector, 224));
        verifier.verify(bytes.concat(j, bytes32(0)), seal);

        // Seals are made before each vm.expectRevert: making one calls the MOCK router, which would take the expectation.
        for (uint256 v; v < 3; v += 2) {
            bytes memory other = abi.encode(v, NU7_BRANCH_ID, anchor, REF, RECEIVER, ZAT);
            bytes memory otherSeal = _seal(other);
            vm.expectRevert(abi.encodeWithSelector(IZcashPaymentVerifier.UnsupportedJournalVersion.selector, v));
            verifier.verify(other, otherSeal);
        }
        bytes memory nu63 = abi.encode(uint8(1), NU6_3_BRANCH_ID, anchor, REF, RECEIVER, ZAT);
        seal = _seal(nu63);
        vm.expectRevert(abi.encodeWithSelector(IZcashPaymentVerifier.BranchNotAllowed.selector, NU6_3_BRANCH_ID));
        verifier.verify(nu63, seal);
        uint256 wide = (uint256(1) << 32) | NU7_BRANCH_ID; // NU7's id in the low 32 bits only
        bytes memory widened = abi.encode(uint8(1), wide, anchor, REF, RECEIVER, ZAT);
        seal = _seal(widened);
        vm.expectRevert(abi.encodeWithSelector(IZcashPaymentVerifier.BranchNotAllowed.selector, wide));
        verifier.verify(widened, seal);
        bytes memory huge = abi.encode(uint8(1), NU7_BRANCH_ID, anchor, REF, RECEIVER, uint256(type(uint64).max) + 1);
        seal = _seal(huge);
        vm.expectRevert(IZcashPaymentVerifier.MalformedJournal.selector);
        verifier.verify(huge, seal);
    }

    function test_verify_refusesSealsOfAnotherSelectorImageOrJournal() public {
        bytes memory j = _journal(anchor, REF, RECEIVER, ZAT);
        vm.expectRevert(abi.encodeWithSelector(IZcashPaymentVerifier.SelectorMismatch.selector, bytes4(0)));
        verifier.verify(j, hex"73c457");
        bytes memory other = abi.encodePacked(bytes4(0x12345678), IMAGE_ID, sha256(j));
        vm.expectRevert(abi.encodeWithSelector(IZcashPaymentVerifier.SelectorMismatch.selector, bytes4(0x12345678)));
        verifier.verify(j, other);
        // The router is asked about the pinned image id and this journal's digest, nothing else.
        bytes memory anotherImage = router.mockSeal(keccak256("another image"), sha256(j));
        vm.expectRevert(MockRiscZeroVerifier.MockSealRejected.selector);
        verifier.verify(j, anotherImage);
        bytes memory anotherJournal = _seal(_journal(anchor, REF, RECEIVER, ZAT + 1));
        vm.expectRevert(MockRiscZeroVerifier.MockSealRejected.selector);
        verifier.verify(j, anotherJournal);
    }

    function test_verify_refusesAnAnchorOffTheBestChain() public {
        bytes32 unknown = keccak256("not a header");
        bytes memory j = _journal(unknown, REF, RECEIVER, ZAT);
        bytes memory seal = _seal(j);
        vm.expectRevert(abi.encodeWithSelector(IZcashPaymentVerifier.AnchorNotInBestChain.selector, unknown));
        verifier.verify(j, seal);
    }

    /// @notice An anchor at the relay's tip is on the best chain with no work on top: the verifier returns 0 and leaves
    ///         finality to the caller.
    function test_verify_returnsZeroWorkAtTheTip() public view {
        bytes32 tip = fx.hashes[fx.hashes.length - 1];
        bytes memory j = _journal(tip, REF, RECEIVER, ZAT);
        (, uint256 work) = verifier.verify(j, _seal(j));
        assertEq(work, 0);
    }

    function test_constructor_refusesInconsistentInput() public {
        IZcashHeaderRelayReader r = IZcashHeaderRelayReader(address(relay));
        IRiscZeroVerifier v = IRiscZeroVerifier(address(router));
        uint32[] memory b = _testnetBranches();
        vm.expectRevert(ZcashPaymentVerifier.ZeroAddress.selector);
        this.deployVerifier(IZcashHeaderRelayReader(address(0)), v, GROTH16_SELECTOR, IMAGE_ID, ZCASH_TESTNET, b);
        vm.expectRevert(ZcashPaymentVerifier.ZeroAddress.selector);
        this.deployVerifier(r, IRiscZeroVerifier(address(0)), GROTH16_SELECTOR, IMAGE_ID, ZCASH_TESTNET, b);
        vm.expectRevert(ZcashPaymentVerifier.ZeroSelectorOrImageId.selector);
        this.deployVerifier(r, v, bytes4(0), IMAGE_ID, ZCASH_TESTNET, b);
        vm.expectRevert(ZcashPaymentVerifier.ZeroSelectorOrImageId.selector);
        this.deployVerifier(r, v, GROTH16_SELECTOR, bytes32(0), ZCASH_TESTNET, b);
        // A Mainnet guest never trusts a Testnet relay.
        vm.expectRevert(
            abi.encodeWithSelector(ZcashPaymentVerifier.WrongNetwork.selector, ZCASH_TESTNET, ZCASH_MAINNET)
        );
        this.deployVerifier(r, v, GROTH16_SELECTOR, IMAGE_ID, ZCASH_MAINNET, b);

        vm.expectRevert(ZcashPaymentVerifier.InvalidBranchIds.selector);
        this.deployVerifier(r, v, GROTH16_SELECTOR, IMAGE_ID, ZCASH_TESTNET, new uint32[](0));
        vm.expectRevert(ZcashPaymentVerifier.InvalidBranchIds.selector);
        this.deployVerifier(r, v, GROTH16_SELECTOR, IMAGE_ID, ZCASH_TESTNET, new uint32[](5));
        uint32[] memory withZero = new uint32[](2);
        withZero[0] = NU7_BRANCH_ID;
        vm.expectRevert(ZcashPaymentVerifier.InvalidBranchIds.selector);
        this.deployVerifier(r, v, GROTH16_SELECTOR, IMAGE_ID, ZCASH_TESTNET, withZero);
    }

    /// @notice A Mainnet verifier on a Mainnet relay allows the two epochs the Mainnet guest walks.
    function test_mainnetVerifierAllowsNu63AndNu7() public {
        ZcashHeaderRelay mainnetRelay = _deploy(_loadReal(MAINNET_RECENT));
        uint32[] memory b = new uint32[](2);
        (b[0], b[1]) = (NU6_3_BRANCH_ID, NU7_BRANCH_ID);
        ZcashPaymentVerifier v = this.deployVerifier(
            IZcashHeaderRelayReader(address(mainnetRelay)),
            IRiscZeroVerifier(address(router)),
            GROTH16_SELECTOR,
            IMAGE_ID,
            ZCASH_MAINNET,
            b
        );
        assertTrue(v.branchAllowed(NU6_3_BRANCH_ID));
        assertTrue(v.branchAllowed(NU7_BRANCH_ID));
        assertFalse(v.branchAllowed(0));
        assertFalse(verifier.branchAllowed(NU6_3_BRANCH_ID), "the Testnet verifier walks NU7 only");
    }

    /// @notice The MOCK router exists only on a local chain.
    function test_mockRouterRefusesOtherChains() public {
        vm.chainId(84532);
        vm.expectRevert(bytes("MOCK: tests and local chains only"));
        new MockRiscZeroVerifier(GROTH16_SELECTOR);
    }
}
