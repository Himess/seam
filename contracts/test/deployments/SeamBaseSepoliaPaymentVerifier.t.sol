// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {Test} from "forge-std/Test.sol";
import {ZCASH_MAINNET, ZCASH_TESTNET} from "../../src/relay/IZcashHeaderRelay.sol";
import {ZcashPayment} from "../../src/payment/IZcashPaymentVerifier.sol";
import {ZcashPaymentVerifier} from "../../src/payment/ZcashPaymentVerifier.sol";
import {DeployPaymentVerifier} from "../../script/DeployPaymentVerifier.s.sol";

/// @notice The payment verifier on Base Sepolia (deployments/seam-base-sepolia.payment-verifier.json): bound to Seam's
///         Testnet relay, RISC Zero's router and the pinned Testnet guest, it verifies the real Testnet payment. With
///         `BASE_SEPOLIA_RPC_URL` the recorded verifier itself is checked on Base Sepolia.
contract SeamBaseSepoliaPaymentVerifierTest is Test {
    uint256 internal constant BASE_SEPOLIA_CHAIN_ID = 84532;
    string internal constant RECORD = "deployments/seam-base-sepolia.payment-verifier.json";
    string internal constant RELAY_RECORD = "deployments/seam-base-sepolia.relay.json";
    string internal constant PAYMENT = "test/examples/fixtures/testnet_real_payment.json";
    address internal constant ROUTER = 0x0b144E07A0826182B6b59788c34b32Bfa86Fb711;
    bytes4 internal constant GROTH16_SELECTOR = 0x73c457ba;
    /// @dev The real payment's reference, value and anchor (header 4,557,390), as its journal binds them.
    bytes32 internal constant PAYMENT_REF = 0x39458b5c2bea9b8e50aa1e413970a4710aa096b57bd98c977fb2d060f973cb9f;
    bytes32 internal constant ANCHOR = 0xcb3ded4678815d97036eb20299090a3054460a97eefc4d711f6cc487c2000000;
    uint64 internal constant ZAT = 1_000_000;

    string internal record;
    string internal payment;

    function setUp() public {
        record = vm.readFile(RECORD);
        payment = vm.readFile(PAYMENT);
    }

    function test_recordBindsTheTestnetRelayAndTheRealPaymentsGuest() public view {
        assertEq(
            vm.parseJsonAddress(record, ".relay"), vm.parseJsonAddress(vm.readFile(RELAY_RECORD), ".ZcashHeaderRelay")
        );
        assertEq(vm.parseJsonAddress(record, ".riscZeroVerifier"), ROUTER);
        assertEq(bytes4(vm.parseJsonBytes(record, ".selector")), GROTH16_SELECTOR);
        assertEq(vm.parseJsonBytes32(record, ".imageId"), vm.parseJsonBytes32(payment, ".imageId"));
        assertEq(vm.parseJsonBytes32(record, ".networkId"), ZCASH_TESTNET);
        assertEq(vm.parseJsonUint(record, ".chainId"), BASE_SEPOLIA_CHAIN_ID);
        uint256[] memory branches = vm.parseJsonUintArray(record, ".branchIds");
        assertEq(branches.length, 1);
        assertEq(branches[0], 0x7719_0AD9);
    }

    function test_branchIdsFollowTheNetwork() public {
        DeployPaymentVerifier script = new DeployPaymentVerifier();
        uint32[] memory t = script.branchIds(ZCASH_TESTNET);
        assertEq(t.length, 1);
        assertEq(t[0], 0x7719_0AD9);
        uint32[] memory m = script.branchIds(ZCASH_MAINNET);
        assertEq(m.length, 2);
        assertEq(m[0], 0x37A5_165B);
        assertEq(m[1], 0x7719_0AD9);
        vm.expectRevert(bytes("DeployPaymentVerifier: not a Zcash network"));
        script.branchIds(keccak256("zcash-regtest"));
    }

    /// @notice Every deployment names its record, and none overwrites another's.
    function test_recordPath() public {
        DeployPaymentVerifier script = new DeployPaymentVerifier();
        assertEq(script.recordPath("anvil"), "deployments/anvil.payment-verifier.json");
        vm.expectRevert(bytes("DeployPaymentVerifier: set DEPLOYMENT_NAME"));
        script.recordPath("");
        vm.expectRevert(
            bytes(
                "DeployPaymentVerifier: deployments/seam-base-sepolia.payment-verifier.json exists: choose another DEPLOYMENT_NAME"
            )
        );
        script.recordPath("seam-base-sepolia");
    }

    /// @notice The recorded verifier verifies the real Testnet payment's seal through RISC Zero's router (skipped
    ///         without `BASE_SEPOLIA_RPC_URL`).
    function test_live_recordedVerifierVerifiesTheRealPayment() public {
        string memory rpc = vm.envOr("BASE_SEPOLIA_RPC_URL", string(""));
        if (bytes(rpc).length == 0) {
            vm.skip(true);
            return;
        }
        vm.createSelectFork(rpc);
        assertEq(block.chainid, BASE_SEPOLIA_CHAIN_ID);
        ZcashPaymentVerifier verifier = ZcashPaymentVerifier(vm.parseJsonAddress(record, ".ZcashPaymentVerifier"));
        assertGt(address(verifier).code.length, 0, "no code at the recorded address");
        assertEq(verifier.relay(), vm.parseJsonAddress(record, ".relay"));
        assertEq(verifier.riscZeroVerifier(), ROUTER);
        assertEq(verifier.selector(), GROTH16_SELECTOR);
        assertEq(verifier.imageId(), vm.parseJsonBytes32(payment, ".imageId"));
        assertEq(verifier.networkId(), ZCASH_TESTNET);
        assertTrue(verifier.branchAllowed(0x7719_0AD9));
        assertFalse(verifier.branchAllowed(0x37A5_165B));

        (ZcashPayment memory p, uint256 anchorWork) =
            verifier.verify(vm.parseJsonBytes(payment, ".journal"), vm.parseJsonBytes(payment, ".seal"));
        assertEq(p.branchId, 0x7719_0AD9);
        assertEq(p.anchor, ANCHOR);
        assertEq(p.paymentRef, PAYMENT_REF);
        assertEq(p.receiverCommitment, vm.parseJsonBytes32(payment, ".receiverCommitment"));
        assertEq(p.zatoshi, ZAT);
        assertGt(anchorWork, 0, "the anchor is on the relay's best chain with work on top");
    }
}
