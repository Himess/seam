// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {PaymentFixtures} from "../payment/PaymentFixtures.sol";
import {MockFeeToken, MockRiscZeroVerifier, MockToken} from "../payment/Mocks.sol";
import {ZcashHeaderRelay} from "../../src/relay/ZcashHeaderRelay.sol";
import {IZcashPaymentVerifier} from "../../src/payment/IZcashPaymentVerifier.sol";
import {ZcashPaymentVerifier} from "../../src/payment/ZcashPaymentVerifier.sol";
import {ZcashFinalityGate} from "../../src/examples/ZcashFinalityGate.sol";
import {ShieldedPaymentEscrow} from "../../src/examples/ShieldedPaymentEscrow.sol";

/// @notice ShieldedPaymentEscrow with a MOCK router and a MOCK token, on a relay fed with real Testnet headers and the
///         synthetic forks of testnet_synthetic.json.
contract ShieldedPaymentEscrowTest is PaymentFixtures {
    uint64 internal constant DEPTH = 6;
    uint64 internal constant DELAY = 150;
    uint64 internal constant GRACE = 1 days;
    uint256 internal constant AMOUNT = 25e6;
    uint64 internal constant ZAT = 2_500_000;
    bytes32 internal constant RECEIVER = keccak256("receiver commitment");
    address internal constant OPENER = address(0xA11CE);
    address internal constant PAYEE = address(0xB0B);

    ZcashHeaderRelay internal relay;
    RealFixture internal fx;
    ZcashPaymentVerifier internal verifier;
    ShieldedPaymentEscrow internal escrow;
    MockToken internal token;
    uint256 internal minWork;

    /// @dev The relay holds R1..R16 of testnet_recent.json; minWork is the work of the 10 headers on top of R6, so R6
    ///      is just final and R1 well past it.
    function setUp() public {
        fx = _loadReal(RECENT);
        relay = _deploy(fx);
        _feed(0, 16);
        minWork = relay.confirmationWork(fx.hashes[5]);
        verifier = _verifier(relay);
        escrow = new ShieldedPaymentEscrow(verifier, minWork, DEPTH, DELAY, GRACE);
        token = new MockToken();
        token.mint(OPENER, 10 * AMOUNT);
        vm.prank(OPENER);
        token.approve(address(escrow), type(uint256).max);
    }

    function _feed(uint256 from, uint256 to) internal {
        for (uint256 i = from; i < to; ++i) {
            _warpForward(fx.headers[i]);
            relay.submitHeader(fx.headers[i]);
        }
    }

    function _open(uint64 deadline) internal returns (bytes32 orderId, bytes32 ref) {
        vm.prank(OPENER);
        (orderId, ref) = escrow.open(IERC20(address(token)), AMOUNT, PAYEE, RECEIVER, ZAT, deadline);
    }

    function _open() internal returns (bytes32 orderId, bytes32 ref) {
        return _open(uint64(block.timestamp + 1 days));
    }

    function _proof(bytes32 anchor, bytes32 ref, bytes32 receiver, uint64 zatoshi)
        internal
        view
        returns (bytes memory journal, bytes memory seal)
    {
        journal = _journal(anchor, ref, receiver, zatoshi);
        seal = _seal(journal);
    }

    /// @notice `paymentRef = keccak256(abi.encode(chainId, escrow, orderId))`; with chain id 84532, escrow 0x1111…11 and
    ///         order 7 it is crates/seam-payment's fixture reference.
    function test_paymentRefVectorIsTheStatementFixtures() public pure {
        bytes32 r = keccak256(
            abi.encode(uint256(84532), address(0x1111111111111111111111111111111111111111), bytes32(uint256(7)))
        );
        assertEq(r, 0x0ffd65bace87c3ecbd455fa36283257624b7a3dcdfaa610f6f37d30dd1cbbf19);
    }

    function test_open_locksExactlyTheAmount() public {
        (bytes32 orderId, bytes32 ref) = _open();
        assertEq(token.balanceOf(address(escrow)), AMOUNT);
        assertEq(ref, keccak256(abi.encode(block.chainid, address(escrow), orderId)));
        assertEq(ref, escrow.paymentRefOf(orderId));
        ShieldedPaymentEscrow.Order memory o = escrow.order(orderId);
        assertEq(o.opener, OPENER);
        assertEq(o.payee, PAYEE);
        assertEq(o.amount, AMOUNT);
        assertEq(o.receiverCommitment, RECEIVER);
        assertEq(o.zatoshi, ZAT);
        assertEq(o.droppedAt, 0);
        assertEq(uint8(o.status), uint8(ShieldedPaymentEscrow.Status.Open));
        assertEq(escrow.lastProofTime(orderId), o.deadline + GRACE);
        (bytes32 second,) = _open();
        assertTrue(second != orderId, "each order has its own id");
    }

    function test_open_refusesBadOrdersAndFeeTokens() public {
        IERC20 t = IERC20(address(token));
        uint64 later = uint64(block.timestamp + 1 days);
        vm.startPrank(OPENER);
        vm.expectRevert(ShieldedPaymentEscrow.InvalidOrder.selector);
        escrow.open(t, 0, PAYEE, RECEIVER, ZAT, later);
        vm.expectRevert(ShieldedPaymentEscrow.InvalidOrder.selector);
        escrow.open(t, AMOUNT, address(0), RECEIVER, ZAT, later);
        vm.expectRevert(ShieldedPaymentEscrow.InvalidOrder.selector);
        escrow.open(t, AMOUNT, PAYEE, bytes32(0), ZAT, later);
        vm.expectRevert(ShieldedPaymentEscrow.InvalidOrder.selector);
        escrow.open(t, AMOUNT, PAYEE, RECEIVER, 0, later);
        vm.expectRevert(ShieldedPaymentEscrow.InvalidOrder.selector);
        escrow.open(t, AMOUNT, PAYEE, RECEIVER, ZAT, uint64(block.timestamp));
        vm.stopPrank();

        MockFeeToken fee = new MockFeeToken();
        fee.mint(OPENER, AMOUNT);
        vm.startPrank(OPENER);
        fee.approve(address(escrow), AMOUNT);
        vm.expectRevert(
            abi.encodeWithSelector(ShieldedPaymentEscrow.TransferMismatch.selector, AMOUNT - AMOUNT / 100, AMOUNT)
        );
        escrow.open(IERC20(address(fee)), AMOUNT, PAYEE, RECEIVER, ZAT, later);
        vm.stopPrank();
    }

    function test_constructor_checksItsInput() public {
        vm.expectRevert(ShieldedPaymentEscrow.ZeroWindow.selector);
        new ShieldedPaymentEscrow(verifier, minWork, 0, DELAY, GRACE);
        vm.expectRevert(ShieldedPaymentEscrow.ZeroWindow.selector);
        new ShieldedPaymentEscrow(verifier, minWork, DEPTH, 0, GRACE);
        vm.expectRevert(ShieldedPaymentEscrow.ZeroWindow.selector);
        new ShieldedPaymentEscrow(verifier, minWork, DEPTH, DELAY, 0);
        vm.expectRevert(ZcashFinalityGate.ZeroMinWork.selector);
        new ShieldedPaymentEscrow(verifier, 0, DEPTH, DELAY, GRACE);
        assertEq(address(escrow.relay()), address(relay));
        assertEq(address(escrow.verifier()), address(verifier));
        assertEq(escrow.proveGrace(), GRACE);
    }

    /// @notice The whole path: a proof starts the pending window, which needs `DEPTH` more headers and `DELAY` seconds,
    ///         then the payee is paid once.
    function test_proveThenFinalizePaysThePayee() public {
        (bytes32 orderId, bytes32 ref) = _open();
        (bytes memory journal, bytes memory seal) = _proof(fx.hashes[0], ref, RECEIVER, ZAT);
        escrow.prove(orderId, journal, seal);
        ShieldedPaymentEscrow.Pending memory p = escrow.pendingSettlement(orderId);
        assertEq(p.anchor, fx.hashes[0]);
        assertEq(p.readyHeight, fx.heights[15] + DEPTH);
        assertEq(p.readyTime, block.timestamp + DELAY);

        vm.expectRevert(abi.encodeWithSelector(ShieldedPaymentEscrow.SettlementAlreadyPending.selector, orderId));
        escrow.prove(orderId, journal, seal);
        vm.expectRevert(
            abi.encodeWithSelector(
                ShieldedPaymentEscrow.SettlementNotReady.selector, orderId, p.readyHeight, p.readyTime
            )
        );
        escrow.finalize(orderId);

        _feed(16, 16 + DEPTH);
        vm.warp(p.readyTime - 1);
        vm.expectRevert(
            abi.encodeWithSelector(
                ShieldedPaymentEscrow.SettlementNotReady.selector, orderId, p.readyHeight, p.readyTime
            )
        );
        escrow.finalize(orderId);
        vm.warp(p.readyTime);
        escrow.finalize(orderId);
        assertEq(token.balanceOf(PAYEE), AMOUNT);
        assertEq(token.balanceOf(address(escrow)), 0);
        assertEq(uint8(escrow.order(orderId).status), uint8(ShieldedPaymentEscrow.Status.Settled));

        vm.expectRevert(abi.encodeWithSelector(ShieldedPaymentEscrow.NoPendingSettlement.selector, orderId));
        escrow.finalize(orderId);
        vm.expectRevert(abi.encodeWithSelector(ShieldedPaymentEscrow.OrderNotOpen.selector, orderId));
        escrow.prove(orderId, journal, seal);
        vm.warp(block.timestamp + 3 days);
        vm.expectRevert(abi.encodeWithSelector(ShieldedPaymentEscrow.OrderNotOpen.selector, orderId));
        escrow.refund(orderId);
    }

    /// @notice Only the payee may take the tokens elsewhere, for a payee that can't receive them.
    function test_finalizeTo_isThePayees() public {
        (bytes32 orderId, bytes32 ref) = _open();
        (bytes memory journal, bytes memory seal) = _proof(fx.hashes[0], ref, RECEIVER, ZAT);
        escrow.prove(orderId, journal, seal);
        _feed(16, 16 + DEPTH);
        vm.warp(escrow.pendingSettlement(orderId).readyTime);
        vm.expectRevert(abi.encodeWithSelector(ShieldedPaymentEscrow.NotPayee.selector, orderId));
        escrow.finalizeTo(orderId, address(this));
        vm.prank(PAYEE);
        vm.expectRevert(ShieldedPaymentEscrow.InvalidOrder.selector);
        escrow.finalizeTo(orderId, address(0));
        vm.prank(PAYEE);
        escrow.finalizeTo(orderId, address(0xCAFE));
        assertEq(token.balanceOf(address(0xCAFE)), AMOUNT);
        assertEq(token.balanceOf(PAYEE), 0);
    }

    function test_prove_refusesAProofOfAnotherPayment() public {
        (bytes32 orderId, bytes32 ref) = _open();
        (bytes32 otherOrder, bytes32 otherRef) = _open();
        bytes32 a = fx.hashes[0];
        (bytes memory j, bytes memory s) = _proof(a, otherRef, RECEIVER, ZAT);
        vm.expectRevert(abi.encodeWithSelector(ShieldedPaymentEscrow.ReferenceMismatch.selector, otherRef));
        escrow.prove(orderId, j, s);
        (j, s) = _proof(a, ref, keccak256("another receiver"), ZAT);
        vm.expectRevert(
            abi.encodeWithSelector(ShieldedPaymentEscrow.ReceiverMismatch.selector, keccak256("another receiver"))
        );
        escrow.prove(orderId, j, s);
        (j, s) = _proof(a, ref, RECEIVER, ZAT - 1);
        vm.expectRevert(abi.encodeWithSelector(ShieldedPaymentEscrow.AmountTooLow.selector, ZAT - 1, ZAT));
        escrow.prove(orderId, j, s);
        // R7 has the work of 9 headers on top: one short of minWork.
        bytes32 recent = fx.hashes[6];
        (j, s) = _proof(recent, ref, RECEIVER, ZAT);
        vm.expectRevert(
            abi.encodeWithSelector(ZcashFinalityGate.NotFinal.selector, recent, relay.confirmationWork(recent), minWork)
        );
        escrow.prove(orderId, j, s);
        // The verifier's own refusals come through.
        (j,) = _proof(a, ref, RECEIVER, ZAT);
        s = router.mockSeal(keccak256("another image"), sha256(j));
        vm.expectRevert(MockRiscZeroVerifier.MockSealRejected.selector);
        escrow.prove(orderId, j, s);
        // An overpayment proves; the other order is untouched.
        (j, s) = _proof(fx.hashes[5], ref, RECEIVER, ZAT + 1);
        escrow.prove(orderId, j, s);
        assertEq(escrow.pendingSettlement(otherOrder).anchor, bytes32(0));
    }

    /// @notice A payment counts by the anchor's header time, not by when it is proven: an anchor after the deadline is
    ///         refused, one before it is proven until `GRACE` after the deadline, and not a second later.
    function test_prove_judgesTimeOnTheZcashSide() public {
        // The order's deadline falls between R16's and R17's header times.
        vm.warp(_time(fx.headers[15]));
        uint64 deadline = uint64(_time(fx.headers[16]) - 1);
        (bytes32 orderId, bytes32 ref) = _open(deadline);
        _feed(16, 32);
        // R17 is final (15 headers on top) but its header comes after the deadline.
        (bytes memory j, bytes memory s) = _proof(fx.hashes[16], ref, RECEIVER, ZAT);
        vm.expectRevert(
            abi.encodeWithSelector(
                ShieldedPaymentEscrow.AnchorAfterDeadline.selector, fx.hashes[16], _time(fx.headers[16]), deadline
            )
        );
        escrow.prove(orderId, j, s);
        // R16 is anchored in time: provable after the deadline, until the grace ends.
        (j, s) = _proof(fx.hashes[15], ref, RECEIVER, ZAT);
        vm.warp(deadline + GRACE + 1);
        vm.expectRevert(
            abi.encodeWithSelector(ShieldedPaymentEscrow.ProofWindowClosed.selector, orderId, deadline + GRACE)
        );
        escrow.prove(orderId, j, s);
        vm.warp(deadline + GRACE);
        escrow.prove(orderId, j, s);
        assertEq(escrow.pendingSettlement(orderId).anchor, fx.hashes[15]);
    }

    function test_refund_onlyOnceTheProofWindowHasClosed() public {
        (bytes32 orderId,) = _open();
        uint64 last = escrow.lastProofTime(orderId);
        vm.warp(last);
        vm.expectRevert(abi.encodeWithSelector(ShieldedPaymentEscrow.ProofWindowOpen.selector, orderId, last));
        escrow.refund(orderId);
        vm.warp(last + 1);
        escrow.refund(orderId);
        assertEq(token.balanceOf(OPENER), 10 * AMOUNT);
        assertEq(uint8(escrow.order(orderId).status), uint8(ShieldedPaymentEscrow.Status.Refunded));
    }

    function test_refund_waitsForAValidPendingSettlement() public {
        (bytes32 orderId, bytes32 ref) = _open();
        (bytes memory j, bytes memory s) = _proof(fx.hashes[0], ref, RECEIVER, ZAT);
        escrow.prove(orderId, j, s);
        vm.warp(escrow.lastProofTime(orderId) + 1);
        vm.expectRevert(abi.encodeWithSelector(ShieldedPaymentEscrow.SettlementIsPending.selector, orderId));
        escrow.refund(orderId);
        // A proof made in time still settles after the window.
        _feed(16, 16 + DEPTH);
        escrow.finalize(orderId);
        assertEq(token.balanceOf(PAYEE), AMOUNT);
    }

    /// @dev A relay at R3 with the synthetic branch A1..A7 on top, and an escrow that requires the work of A2..A7.
    function _relayWithSyntheticBranch() internal returns (bytes32 a1) {
        fx = _loadReal(RECENT);
        relay = _deploy(fx);
        _feed(0, 3);
        string memory json = _synthetic();
        vm.warp(vm.parseJsonUint(json, ".forks.B8.time") + 60);
        for (uint256 i = 1; i <= 7; ++i) {
            (bytes memory raw, bytes32 hash) = _fork(json, string.concat("A", vm.toString(i)));
            relay.submitHeader(raw);
            if (i == 1) a1 = hash;
        }
        verifier = _verifier(relay);
        escrow = new ShieldedPaymentEscrow(verifier, relay.confirmationWork(a1), DEPTH, DELAY, GRACE);
        vm.prank(OPENER);
        token.approve(address(escrow), type(uint256).max);
    }

    /// @notice A pending settlement whose anchor leaves the best chain is dropped by anyone, can no longer be
    ///         finalized, and the payer may prove again on the real chain.
    function test_reorgDropsThePendingSettlement() public {
        bytes32 a1 = _relayWithSyntheticBranch();
        (bytes32 orderId, bytes32 ref) = _open();
        (bytes memory j, bytes memory s) = _proof(a1, ref, RECEIVER, ZAT);
        escrow.prove(orderId, j, s);
        vm.expectRevert(abi.encodeWithSelector(ShieldedPaymentEscrow.SettlementStillValid.selector, orderId));
        escrow.drop(orderId);

        relay.submitHeader(fx.headers[3]); // R4: far heavier than the A branch
        assertFalse(relay.isInBestChain(a1));
        // Past the window (the real chain must reach the readyHeight set at the synthetic tip), finalize refuses the
        // stale anchor.
        _feed(4, 16); // R5..R16: the real chain reaches the height the synthetic branch had set
        vm.warp(escrow.pendingSettlement(orderId).readyTime);
        vm.expectRevert(abi.encodeWithSelector(ZcashFinalityGate.NotFinal.selector, a1, 0, escrow.minWork()));
        escrow.finalize(orderId);

        escrow.drop(orderId);
        assertEq(escrow.pendingSettlement(orderId).anchor, bytes32(0));
        assertEq(escrow.order(orderId).droppedAt, block.timestamp);
        vm.expectRevert(abi.encodeWithSelector(ShieldedPaymentEscrow.NoPendingSettlement.selector, orderId));
        escrow.finalize(orderId);
        vm.expectRevert(abi.encodeWithSelector(IZcashPaymentVerifier.AnchorNotInBestChain.selector, a1));
        escrow.prove(orderId, j, s);
        (j, s) = _proof(fx.hashes[2], ref, RECEIVER, ZAT);
        escrow.prove(orderId, j, s);
        assertEq(escrow.pendingSettlement(orderId).anchor, fx.hashes[2]);
    }

    /// @notice A new proof clears a stale pending settlement itself; a refund never does, so a drop always reopens the
    ///         proof window first: a reorganisation after the deadline can't turn a payment made in time into a refund.
    function test_aStaleSettlementAfterTheDeadlineReopensTheWindow() public {
        bytes32 a1 = _relayWithSyntheticBranch();
        (bytes32 orderId, bytes32 ref) = _open();
        (bytes memory j, bytes memory s) = _proof(a1, ref, RECEIVER, ZAT);
        escrow.prove(orderId, j, s);
        relay.submitHeader(fx.headers[3]); // the anchor leaves the best chain
        uint64 last = escrow.lastProofTime(orderId);
        vm.warp(last + 1);
        vm.expectRevert(abi.encodeWithSelector(ShieldedPaymentEscrow.SettlementIsStale.selector, orderId));
        escrow.refund(orderId);

        // The payer proves again on the real chain: the stale settlement is cleared, the window reopened.
        (j, s) = _proof(fx.hashes[2], ref, RECEIVER, ZAT);
        vm.expectEmit(true, false, false, false, address(escrow));
        emit ShieldedPaymentEscrow.SettlementDropped(orderId);
        escrow.prove(orderId, j, s);
        assertEq(escrow.pendingSettlement(orderId).anchor, fx.hashes[2]);
        assertEq(escrow.order(orderId).droppedAt, block.timestamp);
        assertEq(escrow.lastProofTime(orderId), block.timestamp + GRACE);
    }

    /// @notice A drop reopens the window, and the refund waits for it to close again.
    function test_dropReopensTheProofWindowBeforeARefund() public {
        bytes32 a1 = _relayWithSyntheticBranch();
        (bytes32 orderId, bytes32 ref) = _open();
        (bytes memory j, bytes memory s) = _proof(a1, ref, RECEIVER, ZAT);
        escrow.prove(orderId, j, s);
        relay.submitHeader(fx.headers[3]);
        vm.warp(escrow.lastProofTime(orderId) + 1);
        escrow.drop(orderId);
        uint64 reopened = escrow.lastProofTime(orderId);
        assertEq(reopened, block.timestamp + GRACE);
        vm.expectRevert(abi.encodeWithSelector(ShieldedPaymentEscrow.ProofWindowOpen.selector, orderId, reopened));
        escrow.refund(orderId);
        vm.warp(reopened + 1);
        escrow.refund(orderId);
        assertEq(token.balanceOf(OPENER), 10 * AMOUNT);
    }
}
