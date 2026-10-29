// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {IZcashHeaderRelayReader} from "../interfaces/IZcashHeaderRelayReader.sol";
import {IRiscZeroVerifier} from "./IRiscZeroVerifier.sol";
import {IZcashPaymentVerifier, ZcashPayment} from "./IZcashPaymentVerifier.sol";

/// @title ZcashPaymentVerifier
/// @notice Checks Seam payment proofs for one guest and one relay (docs/PAYMENTS.md). No owner and no upgrade: the
///         relay, the router, the selector, the image id and the allowed branch ids are fixed at deployment.
/// @dev The guest proves the statement of crates/seam-payment for one Zcash network: the constructor checks that the
///      relay follows that network, so a Testnet relay never vouches for a Mainnet guest's anchor, or the reverse. The
///      statement allows the same NU7 branch id on both networks; the relay is what tells them apart.
contract ZcashPaymentVerifier is IZcashPaymentVerifier {
    /// @notice Length of a payment journal: six ABI words.
    uint256 public constant JOURNAL_LENGTH = 6 * 32;
    /// @notice The journal version this verifier reads.
    uint256 public constant JOURNAL_VERSION = 1;
    /// @notice The most branch ids a verifier allows (Mainnet's guest walks two epochs).
    uint256 public constant MAX_BRANCH_IDS = 4;

    /// @inheritdoc IZcashPaymentVerifier
    address public immutable relay;
    /// @inheritdoc IZcashPaymentVerifier
    address public immutable riscZeroVerifier;
    /// @inheritdoc IZcashPaymentVerifier
    bytes4 public immutable selector;
    /// @inheritdoc IZcashPaymentVerifier
    bytes32 public immutable imageId;
    /// @inheritdoc IZcashPaymentVerifier
    bytes32 public immutable networkId;
    // The allowed branch ids; 0 is unused (no epoch with an Ironwood pool has branch id 0).
    uint32 private immutable _branch0;
    uint32 private immutable _branch1;
    uint32 private immutable _branch2;
    uint32 private immutable _branch3;

    error ZeroAddress();
    error ZeroSelectorOrImageId();
    error WrongNetwork(bytes32 relayNetworkId, bytes32 networkId);
    error InvalidBranchIds();

    /// @param relay_ The Seam relay the anchors must be on.
    /// @param riscZeroVerifier_ RISC Zero's verifier router.
    /// @param selector_ The Groth16 selector seals must carry.
    /// @param imageId_ The guest's image id (`payment-test` or `payment-main`).
    /// @param networkId_ The guest's network; the relay must follow it.
    /// @param branchIds The epochs the guest walks: NU7's on Testnet, NU6.3's and NU7's on Mainnet. At most four, none 0.
    constructor(
        IZcashHeaderRelayReader relay_,
        IRiscZeroVerifier riscZeroVerifier_,
        bytes4 selector_,
        bytes32 imageId_,
        bytes32 networkId_,
        uint32[] memory branchIds
    ) {
        if (address(relay_) == address(0) || address(riscZeroVerifier_) == address(0)) {
            revert ZeroAddress();
        }
        if (selector_ == bytes4(0) || imageId_ == bytes32(0)) revert ZeroSelectorOrImageId();
        bytes32 relayNetworkId = relay_.networkId();
        if (relayNetworkId != networkId_) revert WrongNetwork(relayNetworkId, networkId_);
        if (branchIds.length == 0 || branchIds.length > MAX_BRANCH_IDS) revert InvalidBranchIds();
        uint32[MAX_BRANCH_IDS] memory b;
        for (uint256 i; i < branchIds.length; ++i) {
            if (branchIds[i] == 0) revert InvalidBranchIds();
            b[i] = branchIds[i];
        }
        relay = address(relay_);
        riscZeroVerifier = address(riscZeroVerifier_);
        selector = selector_;
        imageId = imageId_;
        networkId = networkId_;
        (_branch0, _branch1, _branch2, _branch3) = (b[0], b[1], b[2], b[3]);
    }

    /// @notice Whether a journal of epoch `branchId` is accepted.
    function branchAllowed(uint256 branchId) public view returns (bool) {
        return
            branchId != 0
                && (branchId == _branch0 || branchId == _branch1 || branchId == _branch2 || branchId == _branch3);
    }

    /// @inheritdoc IZcashPaymentVerifier
    function verify(bytes calldata journal, bytes calldata seal)
        external
        view
        returns (ZcashPayment memory payment, uint256 anchorWork)
    {
        payment = _decode(journal);
        bytes4 sealSelector = seal.length >= 4 ? bytes4(seal[:4]) : bytes4(0);
        if (sealSelector != selector) revert SelectorMismatch(sealSelector);
        IZcashHeaderRelayReader r = IZcashHeaderRelayReader(relay);
        if (!r.isInBestChain(payment.anchor)) revert AnchorNotInBestChain(payment.anchor);
        IRiscZeroVerifier(riscZeroVerifier).verify(seal, imageId, sha256(journal));
        anchorWork = r.confirmationWork(payment.anchor);
    }

    /// @dev `abi.encode(uint8 version, uint32 branchId, bytes32 anchor, bytes32 paymentRef, bytes32 receiverCommitment,
    ///      uint64 zatoshi)`. Every word is read as a uint256 and range-checked, so a malformed journal reverts with a
    ///      named error before the router is called.
    function _decode(bytes calldata journal) internal view returns (ZcashPayment memory p) {
        if (journal.length != JOURNAL_LENGTH) revert InvalidJournalLength(journal.length);
        (uint256 version, uint256 branchId, bytes32 anchor, bytes32 ref, bytes32 receiver, uint256 zatoshi) =
            abi.decode(journal, (uint256, uint256, bytes32, bytes32, bytes32, uint256));
        if (version != JOURNAL_VERSION) revert UnsupportedJournalVersion(version);
        if (!branchAllowed(branchId)) revert BranchNotAllowed(branchId);
        if (zatoshi > type(uint64).max) revert MalformedJournal();
        // Both fit: branchAllowed bounds branchId by the uint32 branch ids, zatoshi is checked above.
        // forge-lint: disable-next-line(unsafe-typecast)
        p = ZcashPayment(uint32(branchId), anchor, ref, receiver, uint64(zatoshi));
    }
}
