// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {SafeERC20} from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import {ReentrancyGuard} from "@openzeppelin/contracts/utils/ReentrancyGuard.sol";
import {IZcashHeaderRelayReader} from "../interfaces/IZcashHeaderRelayReader.sol";
import {IZcashPaymentVerifier, ZcashPayment} from "../payment/IZcashPaymentVerifier.sol";
import {ZcashFinalityGate} from "./ZcashFinalityGate.sol";

/// @title ShieldedPaymentEscrow
/// @notice EXAMPLE, not audited. Locks an ERC-20 until a shielded ZEC payment for the order is proven: the opener locks
///         `amount` of `token` for `payee`, who gets it once someone proves that at least `zatoshi` went, in a shielded
///         Ironwood payment whose memo names the order's `paymentRef`, to the receiver behind `receiverCommitment`
///         (docs/PAYMENTS.md). Without a proof, the opener gets the tokens back.
/// @dev **Timing is judged on the Zcash side.** The payment must be in the chain by the order's `deadline`: its
///      anchor's header time is at most the deadline, and the payment's block comes before the anchor. Proving may take
///      until `proveGrace` after the deadline, or after the last dropped settlement, so a slow prover or a lagging relay
///      never costs a payer who paid in time. A refund waits for that window to close.
///
///      A proof starts a pending window: `finalizeDepth` more Zcash headers and `finalizeDelay` seconds, then the anchor
///      must still be final. A pending settlement whose anchor left the best chain is dropped, by anyone, which reopens
///      the proof window for `proveGrace`. Use a token without transfer fees: `open` refuses one that delivers less than
///      `amount`. If transfers to the payee revert, the payee can take the tokens to another address (`finalizeTo`).
contract ShieldedPaymentEscrow is ZcashFinalityGate, ReentrancyGuard {
    using SafeERC20 for IERC20;

    enum Status {
        None,
        Open,
        Settled,
        Refunded
    }

    struct Order {
        IERC20 token;
        uint256 amount;
        address opener;
        address payee;
        bytes32 receiverCommitment;
        uint64 zatoshi;
        /// @dev The latest header time of a payment's anchor.
        uint64 deadline;
        /// @dev When a settlement of this order was last dropped (0: never).
        uint64 droppedAt;
        Status status;
    }

    struct Pending {
        bytes32 anchor;
        uint64 readyHeight;
        uint64 readyTime;
    }

    /// @notice The payment verifier this escrow trusts.
    IZcashPaymentVerifier public immutable verifier;
    /// @notice Zcash headers that must follow the relay's tip at proof time before a settlement can be finalized.
    uint64 public immutable finalizeDepth;
    /// @notice Seconds that must pass after a proof before its settlement can be finalized.
    uint64 public immutable finalizeDelay;
    /// @notice Seconds to prove a payment made in time, after the deadline or after the last dropped settlement.
    uint64 public immutable proveGrace;

    mapping(bytes32 orderId => Order) private _orders;
    mapping(bytes32 orderId => Pending) private _pending;
    uint256 private _nonce;

    event OrderOpened(
        bytes32 indexed orderId,
        bytes32 paymentRef,
        address indexed token,
        uint256 amount,
        address indexed payee,
        bytes32 receiverCommitment,
        uint64 zatoshi,
        uint64 deadline
    );
    event SettlementPending(bytes32 indexed orderId, bytes32 anchor, uint64 readyHeight, uint64 readyTime);
    event SettlementDropped(bytes32 indexed orderId);
    event OrderSettled(bytes32 indexed orderId, address to);
    event OrderRefunded(bytes32 indexed orderId);

    error ZeroWindow();
    error InvalidOrder();
    error TransferMismatch(uint256 received, uint256 amount);
    error OrderNotOpen(bytes32 orderId);
    error NotPayee(bytes32 orderId);
    error ProofWindowClosed(bytes32 orderId, uint64 lastProofTime);
    error ProofWindowOpen(bytes32 orderId, uint64 lastProofTime);
    error AnchorAfterDeadline(bytes32 anchor, uint256 anchorTime, uint64 deadline);
    error SettlementAlreadyPending(bytes32 orderId);
    error SettlementIsPending(bytes32 orderId);
    error SettlementIsStale(bytes32 orderId);
    error NoPendingSettlement(bytes32 orderId);
    error SettlementStillValid(bytes32 orderId);
    error SettlementNotReady(bytes32 orderId, uint64 readyHeight, uint64 readyTime);
    error ReferenceMismatch(bytes32 paymentRef);
    error ReceiverMismatch(bytes32 receiverCommitment);
    error AmountTooLow(uint64 zatoshi, uint64 required);

    /// @param verifier_ The payment verifier; the gate reads its relay and checks its network.
    /// @param minWork_ Work required on top of a payment's anchor (docs/INTEGRATING.md, "Choosing minWork"). Size it
    ///        against the value all open orders hold together: one reorganisation deep enough can fake payments for every
    ///        order whose receiver and salt the attacker knows. On a Testnet relay no value is safe at any minWork.
    /// @param finalizeDepth_ Headers of the pending window (Testnet example: 6).
    /// @param finalizeDelay_ Seconds of the pending window (Testnet example: 150).
    /// @param proveGrace_ Seconds to prove a payment made in time (example: one day). Keep both windows short enough that
    ///        a settlement can always finish: a pending settlement blocks its order's refund.
    constructor(
        IZcashPaymentVerifier verifier_,
        uint256 minWork_,
        uint64 finalizeDepth_,
        uint64 finalizeDelay_,
        uint64 proveGrace_
    ) ZcashFinalityGate(IZcashHeaderRelayReader(verifier_.relay()), verifier_.networkId(), minWork_) {
        if (finalizeDepth_ == 0 || finalizeDelay_ == 0 || proveGrace_ == 0) revert ZeroWindow();
        verifier = verifier_;
        finalizeDepth = finalizeDepth_;
        finalizeDelay = finalizeDelay_;
        proveGrace = proveGrace_;
    }

    /// @notice Locks exactly `amount` of `token` for `payee` until a shielded payment of at least `zatoshi` to the
    ///         receiver behind `receiverCommitment`, anchored no later than `deadline`, is proven.
    /// @return orderId The order: a hash of the opener, a nonce and the previous block's hash, unique to this order.
    /// @return paymentRef What the payment's memo must name: `seam1:` and its 64 lowercase hex digits.
    function open(
        IERC20 token,
        uint256 amount,
        address payee,
        bytes32 receiverCommitment,
        uint64 zatoshi,
        uint64 deadline
    ) external nonReentrant returns (bytes32 orderId, bytes32 paymentRef) {
        if (amount == 0 || payee == address(0) || receiverCommitment == bytes32(0) || zatoshi == 0) {
            revert InvalidOrder();
        }
        if (deadline <= block.timestamp) revert InvalidOrder();
        orderId = keccak256(abi.encode(msg.sender, _nonce++, blockhash(block.number - 1)));
        paymentRef = paymentRefOf(orderId);
        _orders[orderId] = Order({
            token: token,
            amount: amount,
            opener: msg.sender,
            payee: payee,
            receiverCommitment: receiverCommitment,
            zatoshi: zatoshi,
            deadline: deadline,
            droppedAt: 0,
            status: Status.Open
        });
        emit OrderOpened(orderId, paymentRef, address(token), amount, payee, receiverCommitment, zatoshi, deadline);
        uint256 before = token.balanceOf(address(this));
        // The opener authorized this pull with its allowance.
        // forge-lint: disable-next-line(arbitrary-send-erc20)
        token.safeTransferFrom(msg.sender, address(this), amount);
        uint256 received = token.balanceOf(address(this)) - before;
        if (received != amount) revert TransferMismatch(received, amount);
    }

    /// @notice Proves the order's payment: anyone may call it until the proof window closes. Starts the pending window.
    function prove(bytes32 orderId, bytes calldata journal, bytes calldata seal) external nonReentrant {
        Order storage o = _orders[orderId];
        if (o.status != Status.Open) revert OrderNotOpen(orderId);
        // A valid pending settlement is never replaced; a stale one is dropped first (which reopens the window).
        if (_clearStale(orderId)) revert SettlementAlreadyPending(orderId);
        uint64 last = lastProofTime(orderId);
        if (block.timestamp > last) revert ProofWindowClosed(orderId, last);

        (ZcashPayment memory p,) = verifier.verify(journal, seal);
        if (p.paymentRef != paymentRefOf(orderId)) revert ReferenceMismatch(p.paymentRef);
        if (p.receiverCommitment != o.receiverCommitment) revert ReceiverMismatch(p.receiverCommitment);
        if (p.zatoshi < o.zatoshi) revert AmountTooLow(p.zatoshi, o.zatoshi);
        // The payment's block comes before the anchor, so an anchor no later than the deadline means a payment in time.
        uint256 anchorTime = relay.headerOf(p.anchor).time;
        if (anchorTime > o.deadline) revert AnchorAfterDeadline(p.anchor, anchorTime, o.deadline);
        _requireFinal(p.anchor);

        (, uint256 height,) = relay.bestChainTip();
        // Zcash heights and EVM timestamps fit a uint64 for the chains' lifetimes.
        // forge-lint: disable-next-line(unsafe-typecast)
        uint64 readyHeight = uint64(height) + finalizeDepth;
        // forge-lint: disable-next-line(unsafe-typecast)
        uint64 readyTime = uint64(block.timestamp) + finalizeDelay;
        _pending[orderId] = Pending({anchor: p.anchor, readyHeight: readyHeight, readyTime: readyTime});
        emit SettlementPending(orderId, p.anchor, readyHeight, readyTime);
    }

    /// @notice Pays the order's tokens to its payee once its pending window has passed and its anchor is still final.
    function finalize(bytes32 orderId) external nonReentrant {
        _settle(orderId, _orders[orderId].payee);
    }

    /// @notice As `finalize`, to an address the payee chooses: for a payee that can't receive the token.
    function finalizeTo(bytes32 orderId, address to) external nonReentrant {
        if (msg.sender != _orders[orderId].payee) revert NotPayee(orderId);
        if (to == address(0)) revert InvalidOrder();
        _settle(orderId, to);
    }

    /// @notice Removes a pending settlement whose anchor left the relay's best chain, and reopens the proof window.
    function drop(bytes32 orderId) external nonReentrant {
        Pending storage s = _pending[orderId];
        if (s.anchor == bytes32(0)) revert NoPendingSettlement(orderId);
        if (relay.isInBestChain(s.anchor)) revert SettlementStillValid(orderId);
        _drop(orderId);
    }

    /// @notice Returns the tokens to the opener once the proof window has closed with nothing pending.
    function refund(bytes32 orderId) external nonReentrant {
        Order storage o = _orders[orderId];
        if (o.status != Status.Open) revert OrderNotOpen(orderId);
        uint64 last = lastProofTime(orderId);
        if (block.timestamp <= last) revert ProofWindowOpen(orderId, last);
        bytes32 anchor = _pending[orderId].anchor;
        if (anchor != bytes32(0)) {
            // A proof made in time is resolved first: finalized if it stands; if it doesn't, `drop` reopens the window
            // for the payer to prove again.
            if (relay.isInBestChain(anchor)) revert SettlementIsPending(orderId);
            revert SettlementIsStale(orderId);
        }
        o.status = Status.Refunded;
        emit OrderRefunded(orderId);
        o.token.safeTransfer(o.opener, o.amount);
    }

    /// @notice The last time a proof is accepted: `proveGrace` after the deadline, or after the last dropped settlement.
    function lastProofTime(bytes32 orderId) public view returns (uint64) {
        Order storage o = _orders[orderId];
        uint64 from = o.droppedAt > o.deadline ? o.droppedAt : o.deadline;
        return from + proveGrace;
    }

    /// @notice `keccak256(abi.encode(block.chainid, address(this), orderId))`: the reference the payment names.
    function paymentRefOf(bytes32 orderId) public view returns (bytes32) {
        return keccak256(abi.encode(block.chainid, address(this), orderId));
    }

    function order(bytes32 orderId) external view returns (Order memory) {
        return _orders[orderId];
    }

    function pendingSettlement(bytes32 orderId) external view returns (Pending memory) {
        return _pending[orderId];
    }

    function _settle(bytes32 orderId, address to) internal {
        Pending memory s = _pending[orderId];
        if (s.anchor == bytes32(0)) revert NoPendingSettlement(orderId);
        (, uint256 height,) = relay.bestChainTip();
        if (height < s.readyHeight || block.timestamp < s.readyTime) {
            revert SettlementNotReady(orderId, s.readyHeight, s.readyTime);
        }
        // Still on the best chain with the work required: confirmation work only grows while it stays there.
        _requireFinal(s.anchor);

        Order storage o = _orders[orderId];
        delete _pending[orderId];
        o.status = Status.Settled;
        emit OrderSettled(orderId, to);
        o.token.safeTransfer(to, o.amount);
    }

    /// @dev Drops the order's pending settlement if its anchor left the best chain; true if a valid one remains.
    function _clearStale(bytes32 orderId) internal returns (bool validPending) {
        bytes32 anchor = _pending[orderId].anchor;
        if (anchor == bytes32(0)) return false;
        if (relay.isInBestChain(anchor)) return true;
        _drop(orderId);
        return false;
    }

    function _drop(bytes32 orderId) internal {
        delete _pending[orderId];
        // forge-lint: disable-next-line(unsafe-typecast)
        _orders[orderId].droppedAt = uint64(block.timestamp);
        emit SettlementDropped(orderId);
    }
}
