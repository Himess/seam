// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {Test} from "forge-std/Test.sol";
import {ZcashPow} from "../../src/relay/ZcashPow.sol";

/// @notice ZcashPow against vectors computed with the independent Python model (tools/zcash_header_check.py, a port
///         of zcashd SetCompact/GetCompact and Zebra's threshold_bits).
contract ZcashPowTest is Test {
    uint256 internal constant POW_LIMIT = uint256(0x07ffff) << 232;

    function test_decodeBits_vectors() public pure {
        // invalid: zero, negative, overflow
        _assertInvalid(0x00000000);
        _assertInvalid(0x00123456);
        _assertInvalid(0x01003456);
        _assertInvalid(0x02000056);
        _assertInvalid(0x03000000);
        _assertInvalid(0x04000000);
        _assertInvalid(0x00923456); // negative zero
        _assertInvalid(0x01803456);
        _assertInvalid(0x02800056);
        _assertInvalid(0x03800000);
        _assertInvalid(0x04800000);
        _assertInvalid(0x01fedcba); // negative
        _assertInvalid(0x04923456); // negative
        _assertInvalid(0xff123456); // overflow
        _assertInvalid(0x21010000); // overflow
        _assertInvalid(0x23000001); // overflow
        // valid, and re-encoded as zcashd GetCompact does
        _assertValid(0x01123456, 0x12, 0x01120000);
        _assertValid(0x02123456, 0x1234, 0x02123400);
        _assertValid(0x03123456, 0x123456, 0x03123456);
        _assertValid(0x04123456, 0x12345600, 0x04123456);
        _assertValid(0x05009234, 0x92340000, 0x05009234);
        _assertValid(0x20123456, uint256(0x123456) << 232, 0x20123456);
        _assertValid(0x22000001, uint256(1) << 248, 0x20010000);
        _assertValid(0x2100ffff, uint256(0xffff) << 240, 0x2100ffff);
        _assertValid(0x2007ffff, POW_LIMIT, 0x2007ffff);
        _assertValid(0x1e2c63cf, uint256(0x2c63cf) << 216, 0x1e2c63cf);
    }

    function test_powLimit() public pure {
        assertEq(ZcashPow.encodeBits((uint256(1) << 251) - 1), 0x2007ffff, "ToCompact(2^251 - 1)");
        assertEq(ZcashPow.encodeBits(0), 0);
    }

    function test_work() public pure {
        assertEq(ZcashPow.work(POW_LIMIT), 32);
        (uint256 target,) = ZcashPow.decodeBits(0x1e2c63cf);
        assertEq(ZcashPow.work(target), 377_951);
    }

    function test_thresholdBits_dampingTruncatesTowardsZero() public pure {
        uint256 mean = 1275 << 200;
        // actual - awt = -7: damped = 1275 + (-7 / 4) = 1274 (truncation), not 1273 (floor)
        assertEq(ZcashPow.thresholdBits(mean, 11_268, 10_000, 1275, POW_LIMIT), 0x1b04fa00);
        // actual - awt = +7: damped = 1276
        assertEq(ZcashPow.thresholdBits(mean, 11_282, 10_000, 1275, POW_LIMIT), 0x1b04fc00);
        // on target, NU7 window
        assertEq(ZcashPow.thresholdBits(mean, 12_550, 10_000, 2550, POW_LIMIT), 0x1b04fb00);
    }

    function test_thresholdBits_clampsAndCap() public pure {
        uint256 mean = 1275 << 200;
        // No time elapsed: damped 957 is clamped to MinActualTimespan = 1071 (16 % up).
        assertEq(ZcashPow.thresholdBits(mean, 10_000, 10_000, 1275, POW_LIMIT), 0x1b042f00);
        // A long gap: clamped to MaxActualTimespan = 1683 (32 % down).
        assertEq(ZcashPow.thresholdBits(mean, 1_000_000, 10_000, 1275, POW_LIMIT), 0x1b069300);
        // Never easier than PoWLimit.
        assertEq(ZcashPow.thresholdBits(POW_LIMIT, 1_000_000, 0, 1275, POW_LIMIT), 0x2007ffff);
    }

    function test_median() public pure {
        uint256[] memory t = new uint256[](11);
        uint256[11] memory values = [uint256(9), 3, 7, 1, 11, 5, 2, 10, 4, 8, 6];
        for (uint256 i; i < 11; ++i) {
            t[i] = values[i];
        }
        assertEq(ZcashPow.median(t), 6);
        uint256[] memory dup = new uint256[](3);
        dup[0] = 5;
        dup[1] = 5;
        dup[2] = 1;
        assertEq(ZcashPow.median(dup), 5);
    }

    function test_reverseBytes32() public pure {
        uint256 v = 0x0102030405060708091011121314151617181920212223242526272829303132;
        assertEq(ZcashPow.reverseBytes32(v), 0x3231302928272625242322212019181716151413121110090807060504030201);
    }

    function testFuzz_compactRoundTrip(uint256 target) public pure {
        target = bound(target, 1, POW_LIMIT);
        uint32 bits = ZcashPow.encodeBits(target);
        (uint256 decoded, bool valid) = ZcashPow.decodeBits(bits);
        assertTrue(valid);
        assertLe(decoded, target, "compact form rounds down");
        assertEq(ZcashPow.encodeBits(decoded), bits, "stable");
    }

    function _assertInvalid(uint256 bits) internal pure {
        (, bool valid) = ZcashPow.decodeBits(bits);
        assertFalse(valid);
    }

    function _assertValid(uint256 bits, uint256 target, uint256 reencoded) internal pure {
        (uint256 t, bool valid) = ZcashPow.decodeBits(bits);
        assertTrue(valid);
        assertEq(t, target);
        assertEq(ZcashPow.encodeBits(t), reencoded);
    }
}
