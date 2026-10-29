// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {ERC20} from "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import {IRiscZeroVerifier} from "../../src/payment/IRiscZeroVerifier.sol";

/// @dev Foundry's default chain id: the mocks refuse to exist anywhere else (tests and a local anvil chain).
uint256 constant LOCAL_CHAIN_ID = 31337;

/// @notice MOCK RISC Zero verifier router, for tests only. It accepts exactly the seal
///         `selector || imageId || journalDigest`, so a test sees which image id and journal digest the payment
///         verifier passed. It verifies no proof.
contract MockRiscZeroVerifier is IRiscZeroVerifier {
    bytes4 public immutable selector;

    error MockSealRejected();

    constructor(bytes4 selector_) {
        require(block.chainid == LOCAL_CHAIN_ID, "MOCK: tests and local chains only");
        selector = selector_;
    }

    /// @notice The seal this mock accepts for `imageId` and `journalDigest`.
    function mockSeal(bytes32 imageId, bytes32 journalDigest) public view returns (bytes memory) {
        return abi.encodePacked(selector, imageId, journalDigest);
    }

    function verify(bytes calldata seal, bytes32 imageId, bytes32 journalDigest) external view {
        if (keccak256(seal) != keccak256(mockSeal(imageId, journalDigest))) revert MockSealRejected();
    }
}

/// @notice MOCK ERC-20, for tests only: anyone may mint.
contract MockToken is ERC20 {
    constructor() ERC20("MOCK Token", "MOCK") {
        require(block.chainid == LOCAL_CHAIN_ID, "MOCK: tests and local chains only");
    }

    function mint(address to, uint256 amount) external {
        _mint(to, amount);
    }
}

/// @notice MOCK ERC-20 with a 1 % transfer fee, for tests only: the escrow must refuse it.
contract MockFeeToken is ERC20 {
    constructor() ERC20("MOCK Fee Token", "MOCKFEE") {
        require(block.chainid == LOCAL_CHAIN_ID, "MOCK: tests and local chains only");
    }

    function mint(address to, uint256 amount) external {
        _mint(to, amount);
    }

    function _update(address from, address to, uint256 value) internal override {
        if (from == address(0) || to == address(0)) return super._update(from, to, value);
        uint256 fee = value / 100;
        super._update(from, address(0xdead), fee);
        super._update(from, to, value - fee);
    }
}
