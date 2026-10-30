// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {Test, console2} from "forge-std/Test.sol";
import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {MockToken} from "../payment/Mocks.sol";
import {ZCASH_TESTNET} from "../../src/relay/IZcashHeaderRelay.sol";
import {IZcashHeaderRelayReader} from "../../src/interfaces/IZcashHeaderRelayReader.sol";
import {IRiscZeroVerifier} from "../../src/payment/IRiscZeroVerifier.sol";
import {ZcashPayment} from "../../src/payment/IZcashPaymentVerifier.sol";
import {ZcashPaymentVerifier} from "../../src/payment/ZcashPaymentVerifier.sol";
import {ShieldedPaymentEscrow} from "../../src/examples/ShieldedPaymentEscrow.sol";

/// @notice That the pieces fit, on a Base Sepolia fork: a real shielded payment made on Zcash Testnet and proven on a
///         GPU settles an escrow order, through RISC Zero's real router and Seam's real Testnet relay. Skipped without
///         `BASE_SEPOLIA_RPC_URL`; the settlement waits for the payment's fixture as well.
///
///         The order is the same on every run, so its `paymentRef` was known before the payment was made: this
///         contract deploys the verifier, the MOCK token and the escrow with its first three nonces, the opener is
///         fixed, and the block hash the order id is drawn from is pinned.
contract ShieldedPaymentEscrowForkTest is Test {
    uint256 internal constant BASE_SEPOLIA_CHAIN_ID = 84532;
    /// @dev Seam's Testnet relay (deployments/seam-base-sepolia.relay.json), and RISC Zero's verifier router on Base
    ///      Sepolia with the selector of its Groth16 verifier.
    address internal constant RELAY = 0x77ceDADCFf8bE399C67AB579Ad739820FaC5cE0c;
    address internal constant ROUTER = 0x0b144E07A0826182B6b59788c34b32Bfa86Fb711;
    bytes4 internal constant GROTH16_SELECTOR = 0x73c457ba;
    uint32 internal constant NU7_BRANCH_ID = 0x7719_0AD9;
    /// @dev The payment's fixture: the image id, journal and seal `seam seal` wrote, the Base Sepolia block to prove at
    ///      (the relay holds the anchor with MIN_WORK on top) and a later one to finalize at (FINALIZE_DEPTH more
    ///      headers).
    string internal constant FIXTURE = "test/examples/fixtures/testnet_real_payment.json";

    /// @dev The order the real payment pays: its escrow, id and reference, as test_fork_theOrdersReference first printed
    ///      them (30 October 2026), before the payment named PAYMENT_REF in its memo.
    address internal constant ORDER_ESCROW = 0xF62849F9A0B5Bf2913b396098F7c7019b51A820a;
    bytes32 internal constant ORDER_ID = 0x4b56e40a165fd6b5f6d36b17eeb623d87a0deec5375070c243900c7483fa57f6;
    bytes32 internal constant PAYMENT_REF = 0x39458b5c2bea9b8e50aa1e413970a4710aa096b57bd98c977fb2d060f973cb9f;
    bytes32 internal constant ORDER_BLOCKHASH = keccak256("Seam: the order of the real Testnet payment");
    address internal constant OPENER = address(0xA11CE);
    address internal constant PAYEE = address(0xB0B);
    uint256 internal constant AMOUNT = 25e6;
    uint64 internal constant ZAT = 1_000_000;
    /// @dev About ten Testnet blocks of work after NU7 (docs/INTEGRATING.md § 4).
    uint256 internal constant MIN_WORK = 3_556_000;
    uint64 internal constant FINALIZE_DEPTH = 6;
    uint64 internal constant FINALIZE_DELAY = 150;
    uint64 internal constant PROVE_GRACE = 1 days;

    ZcashPaymentVerifier internal verifier;
    MockToken internal token;
    ShieldedPaymentEscrow internal escrow;

    /// @dev Forks Base Sepolia at `blockNumber` (0: the latest block); false, and the test skipped, without an RPC URL.
    function _fork(uint256 blockNumber) internal returns (bool) {
        string memory rpc = vm.envOr("BASE_SEPOLIA_RPC_URL", string(""));
        if (bytes(rpc).length == 0) {
            vm.skip(true);
            return false;
        }
        if (blockNumber == 0) vm.createSelectFork(rpc);
        else vm.createSelectFork(rpc, blockNumber);
        assertEq(block.chainid, BASE_SEPOLIA_CHAIN_ID);
        return true;
    }

    /// @dev The three deployments, in the order that fixes their addresses, kept across later forks of the same chain.
    function _deploy(bytes32 imageId) internal {
        uint32[] memory branches = new uint32[](1);
        branches[0] = NU7_BRANCH_ID;
        verifier = new ZcashPaymentVerifier(
            IZcashHeaderRelayReader(RELAY),
            IRiscZeroVerifier(ROUTER),
            GROTH16_SELECTOR,
            imageId,
            ZCASH_TESTNET,
            branches
        );
        // The MOCK token exists on local chains only; this fork test is one.
        vm.chainId(31337);
        token = new MockToken();
        vm.chainId(BASE_SEPOLIA_CHAIN_ID);
        escrow = new ShieldedPaymentEscrow(verifier, MIN_WORK, FINALIZE_DEPTH, FINALIZE_DELAY, PROVE_GRACE);
        vm.makePersistent(address(verifier), address(token), address(escrow));
        token.mint(OPENER, AMOUNT);
        vm.prank(OPENER);
        token.approve(address(escrow), AMOUNT);
    }

    function _open(bytes32 receiverCommitment) internal returns (bytes32 orderId, bytes32 ref) {
        vm.setBlockhash(block.number - 1, ORDER_BLOCKHASH);
        vm.prank(OPENER);
        (orderId, ref) = escrow.open(
            IERC20(address(token)), AMOUNT, PAYEE, receiverCommitment, ZAT, uint64(block.timestamp + PROVE_GRACE)
        );
    }

    /// @notice The order's escrow, id and reference, the same on every run: the reference the payment's memo names.
    function test_fork_theOrdersReference() public {
        if (!_fork(0)) return;
        _deploy(keccak256("any image id: the addresses do not depend on it"));
        (bytes32 orderId, bytes32 ref) = _open(keccak256("any receiver: the reference does not depend on it"));
        console2.log("escrow", address(escrow));
        console2.log("orderId");
        console2.logBytes32(orderId);
        console2.log("paymentRef");
        console2.logBytes32(ref);
        assertEq(orderId, keccak256(abi.encode(OPENER, uint256(0), ORDER_BLOCKHASH)));
        assertEq(ref, keccak256(abi.encode(BASE_SEPOLIA_CHAIN_ID, address(escrow), orderId)));
        assertEq(address(escrow), ORDER_ESCROW);
        assertEq(orderId, ORDER_ID);
        assertEq(ref, PAYMENT_REF);
    }

    /// @notice The real payment, proven, settles its order: `prove` verifies the real seal through the real router and
    ///         finds the anchor in the real relay with MIN_WORK on top; `finalize` pays the payee FINALIZE_DEPTH
    ///         headers later.
    function test_fork_theRealPaymentSettlesItsOrder() public {
        if (!vm.exists(FIXTURE)) {
            vm.skip(true);
            return;
        }
        string memory json = vm.readFile(FIXTURE);
        bytes memory journal = vm.parseJsonBytes(json, ".journal");
        bytes memory seal = vm.parseJsonBytes(json, ".seal");
        if (!_fork(vm.parseJsonUint(json, ".proveBlock"))) return;
        _deploy(vm.parseJsonBytes32(json, ".imageId"));

        uint256 gasBefore = gasleft();
        (ZcashPayment memory p, uint256 anchorWork) = verifier.verify(journal, seal);
        console2.log("verify gas, with the Groth16 check", gasBefore - gasleft());
        assertEq(p.branchId, NU7_BRANCH_ID);
        assertGe(p.zatoshi, ZAT);
        assertGe(anchorWork, MIN_WORK);

        (bytes32 orderId, bytes32 ref) = _open(p.receiverCommitment);
        assertEq(ref, PAYMENT_REF);
        assertEq(p.paymentRef, ref, "the payment names the order");
        escrow.prove(orderId, journal, seal);
        ShieldedPaymentEscrow.Pending memory pending = escrow.pendingSettlement(orderId);
        assertEq(pending.anchor, p.anchor);

        vm.rollFork(vm.parseJsonUint(json, ".finalizeBlock"));
        (, uint256 tip,) = IZcashHeaderRelayReader(RELAY).bestChainTip();
        assertGe(tip, pending.readyHeight, "the finalize block's relay holds FINALIZE_DEPTH more headers");
        if (block.timestamp < pending.readyTime) vm.warp(pending.readyTime);
        escrow.finalize(orderId);
        assertEq(token.balanceOf(PAYEE), AMOUNT);
        assertEq(uint8(escrow.order(orderId).status), uint8(ShieldedPaymentEscrow.Status.Settled));
    }
}
