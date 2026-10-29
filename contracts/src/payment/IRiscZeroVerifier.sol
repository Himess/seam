// SPDX-License-Identifier: MIT
pragma solidity ^0.8.4;

/// @title IRiscZeroVerifier
/// @notice The one function of RISC Zero's verifier interface (risc0-ethereum 3.0, `IRiscZeroVerifier`) that the payment
///         verifier calls. RISC Zero's `RiscZeroVerifierRouter` implements it and dispatches on the seal's first four
///         bytes to the verifier registered under that selector.
interface IRiscZeroVerifier {
    /// @notice Returns normally only if `seal` proves that the zkVM program `imageId` ran to completion and committed a
    ///         journal whose SHA-256 is `journalDigest`; reverts otherwise.
    function verify(bytes calldata seal, bytes32 imageId, bytes32 journalDigest) external view;
}
