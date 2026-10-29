// SPDX-License-Identifier: MIT
pragma solidity ^0.8.4;

import {IZcashHeaderRelayReader} from "../interfaces/IZcashHeaderRelayReader.sol";

/// @title ZcashFinalityGate
/// @notice EXAMPLE, not audited. Lets a contract act only on a Zcash block that is on the relay's best chain with at
///         least `minWork` of proof of work on top of it. It requires work, not a block count: on Testnet,
///         minimum-difficulty blocks make block counts almost free (docs/INTEGRATING.md).
/// @dev The check holds at the moment of use. A heavier branch can later take a block off the best chain, so a contract
///      that records "final" must pick a `minWork` that makes such a reorganisation uneconomic.
abstract contract ZcashFinalityGate {
    /// @notice The relay this gate reads.
    IZcashHeaderRelayReader public immutable relay;
    /// @notice Work required on top of a block, fixed at deployment (docs/INTEGRATING.md, "Choosing minWork").
    uint256 public immutable minWork;

    error ZeroMinWork();
    error WrongNetwork(bytes32 networkId);
    error NotFinal(bytes32 blockHash, uint256 work, uint256 required);

    constructor(IZcashHeaderRelayReader relay_, bytes32 expectedNetworkId, uint256 minWork_) {
        if (minWork_ == 0) revert ZeroMinWork();
        bytes32 id = relay_.networkId();
        if (id != expectedNetworkId) revert WrongNetwork(id);
        relay = relay_;
        minWork = minWork_;
    }

    /// @notice True if `blockHash` is on the best chain with at least `minWork` of work on top of it.
    function isFinal(bytes32 blockHash) public view returns (bool) {
        return relay.confirmationWork(blockHash) >= minWork;
    }

    /// @dev Reverts with `NotFinal` unless `blockHash` is final.
    function _requireFinal(bytes32 blockHash) internal view {
        uint256 work = relay.confirmationWork(blockHash);
        if (work < minWork) revert NotFinal(blockHash, work, minWork);
    }
}
