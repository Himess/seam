// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {ZcashEquihash} from "./ZcashEquihash.sol";

/// @title IZcashHeaderRelay
/// @notice A Zcash Testnet block-header relay: proof-of-work-verified headers from a trusted checkpoint, best chain
///         by cumulative work.
/// @dev Block hashes are SHA256d of the 1487-byte header in internal byte order (the bytes32 that appears in the
///      next header's hashPrevBlock). Block explorers display them byte-reversed.
interface IZcashHeaderRelay {
    /// @notice Stored data of a known header.
    struct HeaderInfo {
        bool known;
        bytes32 prevHash;
        uint256 height;
        uint256 time;
        uint256 bits;
        /// @dev Cumulative work from the checkpoint (the checkpoint itself counts 0).
        uint256 chainWork;
        bytes32 merkleRoot;
        /// @dev hashBlockCommitments (ZIP 244): commits to the ZIP 221 chain history root, which commits to the
        ///      Ironwood note commitment tree roots (ZIP 258).
        bytes32 blockCommitments;
    }

    /// @notice A header passed every check and was stored.
    event HeaderAdded(bytes32 indexed blockHash, bytes32 indexed prevHash, uint256 height, uint256 chainWork);

    /// @notice The best chain tip changed. A reorganisation happened when `forkHeight` < the previous tip's height:
    ///         the blocks of the previous best chain above `forkHeight` left the best chain.
    event BestChainUpdated(bytes32 indexed newTip, uint256 newHeight, bytes32 indexed previousTip, uint256 forkHeight);

    error InvalidCheckpoint();
    error InvalidContextLength();
    error InvalidContextBits(uint256 index);
    error InvalidHeaderLength(uint256 length);
    error InvalidSolutionLength();
    error InvalidVersion(int256 version);
    error BlockAlreadyKnown(bytes32 blockHash);
    error UnknownParent(bytes32 prevHash);
    error TimeTooEarly(uint256 time, uint256 medianTimePast);
    error TimeTooLate(uint256 time, uint256 maxTime);
    error TimeTooFarInFuture(uint256 time, uint256 maxTime);
    error BadDifficultyBits(uint256 bits, uint256 expected);
    error InvalidEquihashSolution(ZcashEquihash.Result result);
    error HashAboveTarget(bytes32 blockHash, uint256 target);

    /// @notice Verifies and stores one header (1487 bytes). Reverts if any check fails.
    function submitHeader(bytes calldata header) external returns (bytes32 blockHash);

    /// @notice Submits several headers in order, parents first.
    function submitHeaders(bytes[] calldata headers) external;

    /// @notice True if the block is on the current best chain.
    function isInBestChain(bytes32 blockHash) external view returns (bool);

    /// @notice Number of best-chain blocks from this block to the tip, inclusive (1 = the tip); 0 if the block is
    ///         not on the best chain. On Testnet see `confirmationWork`.
    function confirmations(bytes32 blockHash) external view returns (uint256);

    /// @notice Cumulative work of the best-chain blocks built on top of this block; 0 if it is not on the best
    ///         chain. Testnet minimum-difficulty blocks add a block but almost no work, so a consumer that needs
    ///         reorg resistance on Testnet should require work, not only a block count.
    function confirmationWork(bytes32 blockHash) external view returns (uint256);

    /// @notice Stored fields of a header (`known` = false if unknown). The checkpoint has no prevHash, merkleRoot or
    ///         blockCommitments (zero).
    function headerOf(bytes32 blockHash) external view returns (HeaderInfo memory);

    /// @notice The best chain tip.
    function bestChainTip() external view returns (bytes32 blockHash, uint256 height, uint256 chainWork);

    /// @notice Best-chain block hash at `height`; zero above the tip or below the checkpoint.
    function bestBlockHashAt(uint256 height) external view returns (bytes32);
}
