// SPDX-License-Identifier: MIT
pragma solidity ^0.8.4;

/// @notice A shielded Ironwood payment, as a Seam payment proof's journal states it (docs/PAYMENTS.md).
struct ZcashPayment {
    /// @dev Consensus branch ID of the epoch the proof walked.
    uint32 branchId;
    /// @dev SHA-256d of header T, internal byte order (the relay's block hash). The payment is in a block of T's epoch
    ///      at or below T.
    bytes32 anchor;
    /// @dev The reference the note's memo names (`seam1:` and its 64 lowercase hex digits).
    bytes32 paymentRef;
    /// @dev `keccak256(salt32 || receiver43)` of the note's receiver.
    bytes32 receiverCommitment;
    /// @dev The note's value, in zatoshi.
    uint64 zatoshi;
}

/// @title IZcashPaymentVerifier
/// @notice Checks a Seam payment proof: a RISC Zero receipt, wrapped in Groth16, that the pinned guest accepted a
///         shielded payment whose anchor is on a Seam relay's best chain.
interface IZcashPaymentVerifier {
    error InvalidJournalLength(uint256 length);
    error MalformedJournal();
    error UnsupportedJournalVersion(uint256 version);
    error BranchNotAllowed(uint256 branchId);
    error SelectorMismatch(bytes4 selector);
    error AnchorNotInBestChain(bytes32 anchor);

    /// @notice The Seam relay the anchor must be on.
    function relay() external view returns (address);

    /// @notice The RISC Zero verifier router.
    function riscZeroVerifier() external view returns (address);

    /// @notice The Groth16 selector a seal must carry.
    function selector() external view returns (bytes4);

    /// @notice The pinned guest (zkvm/pinned-image-ids.txt).
    function imageId() external view returns (bytes32);

    /// @notice The Zcash network of the guest and the relay (`ZcashNetworks.TESTNET` or `ZcashNetworks.MAINNET`).
    function networkId() external view returns (bytes32);

    /// @notice Reverts unless: the journal is 192 bytes, version 1, a branch id this verifier allows; the seal's first 4
    ///         bytes are the pinned selector; the router verifies (seal, imageId, sha256(journal)); the anchor is on the
    ///         relay's best chain. Returns the payment and the work on top of its anchor; finality is the caller's
    ///         choice.
    function verify(bytes calldata journal, bytes calldata seal)
        external
        view
        returns (ZcashPayment memory payment, uint256 anchorWork);
}
