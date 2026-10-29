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
///         (docs/PAYMENTS.md). Without a proof by the deadline, the opener gets the tokens back.
/// @dev A proof starts a pending window: `finalizeDepth` more Zcash headers and `finalizeDelay` seconds, then the anchor
///      must still be final. A pending settlement whose anchor left the best chain is dropped, by anyone, and a new
///      proof may follow. Use a token without transfer fees: `open` refuses one that delivers less than `amount`.
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
        uint64 deadline;
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
    event OrderSettled(bytes32 indexed orderId);
    event OrderRefunded(bytes32 indexed orderId);

    error ZeroWindow();
    error InvalidOrder();
    error TransferMismatch(uint256 received, uint256 amount);
    error OrderNotOpen(bytes32 orderId);
    error DeadlinePassed(bytes32 orderId, uint64 deadline);
    error DeadlineNotPassed(bytes32 orderId, uint64 deadline);
    error SettlementAlreadyPending(bytes32 orderId);
    error SettlementIsPending(bytes32 orderId);
    error NoPendingSettlement(bytes32 orderId);
    error SettlementStillValid(bytes32 orderId);
    error SettlementNotReady(bytes32 orderId, uint64 readyHeight, uint64 readyTime);
    error ReferenceMismatch(bytes32 paymentRef);
    error ReceiverMismatch(bytes32 receiverCommitment);
    error AmountTooLow(uint64 zatoshi, uint64 required);

    /// @param verifier_ The payment verifier; the gate reads its relay and checks its network.
    /// @param minWork_ Work required on top of a payment's anchor (docs/INTEGRATING.md, "Choosing minWork").
    /// @param finalizeDepth_ Headers of the pending window (Testnet example: 6).
    /// @param finalizeDelay_ Seconds of the pending window (Testnet example: 150).
    constructor(IZcashPaymentVerifier verifier_, uint256 minWork_, uint64 finalizeDepth_, uint64 finalizeDelay_)
        ZcashFinalityGate(IZcashHeaderRelayReader(verifier_.relay()), verifier_.networkId(), minWork_)
    {
        if (finalizeDepth_ == 0 || finalizeDelay_ == 0) revert ZeroWindow();
        verifier = verifier_;
        finalizeDepth = finalizeDepth_;
        finalizeDelay = finalizeDelay_;
    }

    /// @notice Locks exactly `amount` of `token` for `payee` until a shielded payment of at least `zatoshi` to the
    ///         receiver behind `receiverCommitment` is proven, or `deadline` passes.
    /// @return orderId The order. Unpredictable before this call: it depends on the previous block's hash.
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

    /// @notice Proves the order's payment: anyone may call it before the deadline. Starts the pending window.
    function prove(bytes32 orderId, bytes calldata journal, bytes calldata seal) external nonReentrant {
        Order storage o = _orders[orderId];
        if (o.status != Status.Open) revert OrderNotOpen(orderId);
        if (block.timestamp > o.deadline) revert DeadlinePassed(orderId, o.deadline);
        // A valid pending settlement is never replaced; a stale one no longer blocks an honest proof.
        if (_clearStale(orderId)) revert SettlementAlreadyPending(orderId);

        (ZcashPayment memory p,) = verifier.verify(journal, seal);
        if (p.paymentRef != paymentRefOf(orderId)) revert ReferenceMismatch(p.paymentRef);
        if (p.receiverCommitment != o.receiverCommitment) revert ReceiverMismatch(p.receiverCommitment);
        if (p.zatoshi < o.zatoshi) revert AmountTooLow(p.zatoshi, o.zatoshi);
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
        emit OrderSettled(orderId);
        o.token.safeTransfer(o.payee, o.amount);
    }

    /// @notice Removes a pending settlement whose anchor left the relay's best chain.
    function drop(bytes32 orderId) external nonReentrant {
        Pending storage s = _pending[orderId];
        if (s.anchor == bytes32(0)) revert NoPendingSettlement(orderId);
        if (relay.isInBestChain(s.anchor)) revert SettlementStillValid(orderId);
        delete _pending[orderId];
        emit SettlementDropped(orderId);
    }

    /// @notice Returns the tokens to the opener after the deadline, if no valid settlement is pending.
    function refund(bytes32 orderId) external nonReentrant {
        Order storage o = _orders[orderId];
        if (o.status != Status.Open) revert OrderNotOpen(orderId);
        if (block.timestamp <= o.deadline) revert DeadlineNotPassed(orderId, o.deadline);
        // A proof made in time is resolved first: finalized if it stands, dropped (here) if not.
        if (_clearStale(orderId)) revert SettlementIsPending(orderId);
        o.status = Status.Refunded;
        emit OrderRefunded(orderId);
        o.token.safeTransfer(o.opener, o.amount);
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

    /// @dev Drops the order's pending settlement if its anchor left the best chain; true if a valid one remains.
    function _clearStale(bytes32 orderId) internal returns (bool validPending) {
        Pending storage s = _pending[orderId];
        if (s.anchor == bytes32(0)) return false;
        if (relay.isInBestChain(s.anchor)) return true;
        delete _pending[orderId];
        emit SettlementDropped(orderId);
        return false;
    }
}
