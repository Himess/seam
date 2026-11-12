// SPDX-License-Identifier: MIT
pragma solidity ^0.8.4;

/// @notice A shielded Ironwood payment, as a Seam amount-hidden payment proof's journal (version 2) states it: the
///         payment statement of `ZcashPayment`, with the amount replaced by the app's masked value M. The guest proved
///         that the note pays at least `M - R` (mod 2^64), where R is the first 8 bytes of
///         keccak256("seam/amount-mask/v1" || salt32 || paymentRef) and the salt is the one `receiverCommitment` binds
///         (crates/seam-payment/src/masked.rs).
struct ZcashMaskedPayment {
    /// @dev Consensus branch ID of the epoch the proof walked.
    uint32 branchId;
    /// @dev SHA-256d of header T, internal byte order (the relay's block hash).
    bytes32 anchor;
    /// @dev The reference the note's memo names (`seam1:` and its 64 lowercase hex digits).
    bytes32 paymentRef;
    /// @dev `keccak256(salt32 || receiver43)` of the note's receiver.
    bytes32 receiverCommitment;
    /// @dev M, as the app published it. An app accepts the proof only if this is its own M.
    uint64 maskedAmount;
}

/// @title IZcashMaskedPaymentVerifier
/// @notice Checks a Seam amount-hidden payment proof: a RISC Zero receipt, wrapped in Groth16, that the pinned guest
///         accepted a shielded payment of at least `M - R` whose anchor is on a Seam relay's best chain.
interface IZcashMaskedPaymentVerifier {
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

    /// @notice The pinned amount-hidden guest (zkvm/pinned-image-ids.txt: payment-masked-test or payment-masked-main).
    function imageId() external view returns (bytes32);

    /// @notice The Zcash network of the guest and the relay (`ZcashNetworks.TESTNET` or `ZcashNetworks.MAINNET`).
    function networkId() external view returns (bytes32);

    /// @notice Reverts unless: the journal is 192 bytes, version 2, a branch id this verifier allows; the seal's first 4
    ///         bytes are the pinned selector; the router verifies (seal, imageId, sha256(journal)); the anchor is on the
    ///         relay's best chain. Returns the payment and the work on top of its anchor; finality, deadlines and the
    ///         check of `maskedAmount` against the app's own M are the caller's.
    function verify(bytes calldata journal, bytes calldata seal)
        external
        view
        returns (ZcashMaskedPayment memory payment, uint256 anchorWork);
}
