// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {console} from "forge-std/console.sol";
import {RelayFixtures} from "./RelayFixtures.sol";
import {ZcashEquihash} from "../../src/relay/ZcashEquihash.sol";

contract EquihashHarness {
    function verify(bytes memory header) external view returns (ZcashEquihash.Result) {
        return ZcashEquihash.verify(header);
    }

    function leafNodes(bytes memory header) external view returns (bool distinct, uint256[] memory nodes) {
        nodes = new uint256[](512);
        distinct = ZcashEquihash.leafNodes(header, nodes);
    }

    function checkTree(uint256[] memory nodes) external pure returns (ZcashEquihash.Result) {
        return ZcashEquihash.checkTree(nodes);
    }
}

/// @notice ZcashEquihash against real Testnet solutions, Python-computed BLAKE2b leaves and crafted trees.
contract ZcashEquihashTest is RelayFixtures {
    EquihashHarness internal harness;
    uint256 internal constant MASK200 = (uint256(1) << 200) - 1;

    function setUp() public {
        harness = new EquihashHarness();
    }

    /// @notice The hard-coded initial state equals IV XOR parameter block (RFC 7693 § 2.5) for digest length 50
    ///         and personalization "ZcashPoW" || le32(200) || le32(9).
    function test_initialState() public pure {
        uint64[8] memory iv = [
            0x6a09e667f3bcc908,
            0xbb67ae8584caa73b,
            0x3c6ef372fe94f82b,
            0xa54ff53a5f1d36f1,
            0x510e527fade682d1,
            0x9b05688c2b3e6c1f,
            0x1f83d9abfb41bd6b,
            0x5be0cd19137e2179
        ];
        uint64[8] memory p;
        p[0] = 0x01010032; // digest length 50, key length 0, fanout 1, depth 1
        p[6] = _le64(bytes8("ZcashPoW"));
        p[7] = 200 | (uint64(9) << 32); // le32(200) || le32(9) read as a little-endian u64
        bytes memory state;
        for (uint256 i; i < 8; ++i) {
            state = bytes.concat(state, _toLe64(iv[i] ^ p[i]));
        }
        assertEq(_bytes32At(state, 0), ZcashEquihash.H_INIT_0);
        assertEq(_bytes32At(state, 32), ZcashEquihash.H_INIT_1);
    }

    /// @notice EIP-152 test vector 5: one 12-round final compression of "abc" gives BLAKE2b-512("abc").
    function test_precompile_eip152Vector() public view {
        bytes memory input = bytes.concat(
            hex"0000000c",
            hex"48c9bdf267e6096a3ba7ca8485ae67bb2bf894fe72f36e3cf1361d5f3af54fa5",
            hex"d182e6ad7f520e511f6c3e2b8c68059b6bbd41fbabd9831f79217e1319cde05b",
            hex"6162630000000000000000000000000000000000000000000000000000000000",
            new bytes(96),
            hex"0300000000000000",
            hex"0000000000000000",
            hex"01"
        );
        assertEq(input.length, 213);
        (bool ok, bytes memory out) = address(0x09).staticcall(input);
        assertTrue(ok);
        assertEq(
            out,
            hex"ba80a53f981c4d0d6a2797b69f12f6e94c212f14685ac4b74b12bb6fdbffa2d17d87c5392aab792dc252d5de4533cc9518d38aa8dbf1925ab92386edd4009923"
        );
    }

    /// @notice The first leaves of header 4,488,000 equal those computed with Python's hashlib BLAKE2b.
    function test_leafHashesMatchPythonBlake2b() public view {
        RealFixture memory fx = _loadReal(RECENT);
        (bool distinct, uint256[] memory nodes) = harness.leafNodes(fx.headers[0]);
        assertTrue(distinct);
        string memory json = _synthetic();
        for (uint256 k; k < 4; ++k) {
            string memory key = string.concat(".leaf_vectors[", vm.toString(k), "]");
            uint256 index = vm.parseJsonUint(json, string.concat(key, ".index"));
            bytes memory leaf = vm.parseJsonBytes(json, string.concat(key, ".leaf"));
            assertEq(leaf.length, 25);
            assertEq(nodes[k] >> 224, index, "index");
            assertEq(nodes[k] & MASK200, uint256(_bytes32At(leaf, 0)) >> 56, "leaf");
        }
    }

    /// @notice All 62 real Testnet solutions of the two real fixtures are valid.
    function test_realSolutionsAreValid() public view {
        string[2] memory paths = [RECENT, NU7_ACTIVATION];
        uint256 count;
        for (uint256 p; p < 2; ++p) {
            RealFixture memory fx = _loadReal(paths[p]);
            for (uint256 i; i < fx.headers.length; ++i) {
                assertEq(uint8(harness.verify(fx.headers[i])), uint8(ZcashEquihash.Result.Valid));
                ++count;
            }
        }
        assertEq(count, 62);
    }

    function test_mutationsAreRejectedWithTheReferenceKind() public view {
        string memory json = _synthetic();
        string[6] memory labels = [
            "solution_bit_flip",
            "nonce_bit_flip",
            "merkle_root_bit_flip",
            "swapped_leaves",
            "swapped_subtrees",
            "duplicated_half"
        ];
        ZcashEquihash.Result[6] memory kinds = [
            ZcashEquihash.Result.Collision,
            ZcashEquihash.Result.Collision,
            ZcashEquihash.Result.Collision,
            ZcashEquihash.Result.OutOfOrder,
            ZcashEquihash.Result.OutOfOrder,
            ZcashEquihash.Result.DuplicateIdxs
        ];
        for (uint256 i; i < 6; ++i) {
            bytes memory raw = vm.parseJsonBytes(json, string.concat(".mutations.", labels[i], ".raw"));
            assertEq(uint8(harness.verify(raw)), uint8(kinds[i]), labels[i]);
        }
    }

    function test_invalidEncoding() public view {
        RealFixture memory fx = _loadReal(RECENT);
        bytes memory h = _copy(fx.headers[0]);
        h[142] = 0x06;
        assertEq(uint8(harness.verify(h)), uint8(ZcashEquihash.Result.InvalidEncoding));
        assertEq(uint8(harness.verify(new bytes(1486))), uint8(ZcashEquihash.Result.InvalidEncoding));
        assertEq(uint8(harness.checkTree(new uint256[](511))), uint8(ZcashEquihash.Result.InvalidEncoding));
    }

    // --- crafted trees (leaf strings chosen directly, BLAKE2b bypassed) ---

    function test_checkTree_crafted() public view {
        uint256[] memory nodes = _ascending();
        assertEq(uint8(harness.checkTree(nodes)), uint8(ZcashEquihash.Result.Valid), "all-zero leaves");

        nodes = _ascending();
        nodes[0] |= 1; // only the last 20-bit chunk is non-zero: every collision holds, the root does not vanish
        assertEq(uint8(harness.checkTree(nodes)), uint8(ZcashEquihash.Result.NonZeroRootHash));

        nodes = _ascending();
        nodes[0] |= uint256(1) << 199; // chunk 0 differs inside the first pair
        assertEq(uint8(harness.checkTree(nodes)), uint8(ZcashEquihash.Result.Collision));

        nodes = _ascending();
        nodes[5] |= uint256(1) << 40; // chunk 7: the pairs up to level 7 collide, level 8 does not
        assertEq(uint8(harness.checkTree(nodes)), uint8(ZcashEquihash.Result.Collision));

        nodes = _ascending();
        (nodes[0], nodes[1]) = (nodes[1], nodes[0]); // first pair out of order
        assertEq(uint8(harness.checkTree(nodes)), uint8(ZcashEquihash.Result.OutOfOrder));

        nodes = _ascending();
        for (uint256 i; i < 256; ++i) {
            (nodes[i], nodes[i + 256]) = (nodes[i + 256], nodes[i]); // only the top-level merge is out of order
        }
        assertEq(uint8(harness.checkTree(nodes)), uint8(ZcashEquihash.Result.OutOfOrder));
    }

    /// @notice Any single bit flip in a real solution is rejected.
    function testFuzz_solutionBitFlipIsRejected(uint256 bit) public view {
        RealFixture memory fx = _loadReal(RECENT);
        bit = bound(bit, 0, 1344 * 8 - 1);
        bytes memory h = _copy(fx.headers[0]);
        h[143 + bit / 8] ^= bytes1(uint8(1 << (bit % 8)));
        assertTrue(harness.verify(h) != ZcashEquihash.Result.Valid);
    }

    function test_gas_verifyOneSolution() public view {
        RealFixture memory fx = _loadReal(RECENT);
        uint256 g = gasleft();
        harness.verify(fx.headers[0]);
        console.log("Equihash (200, 9) verification gas, including the call:", g - gasleft());
    }

    function _ascending() internal pure returns (uint256[] memory nodes) {
        nodes = new uint256[](512);
        for (uint256 i; i < 512; ++i) {
            nodes[i] = i << 224;
        }
    }

    function _le64(bytes8 b) internal pure returns (uint64 v) {
        for (uint256 i; i < 8; ++i) {
            v |= uint64(uint8(b[i])) << uint64(8 * i);
        }
    }

    function _toLe64(uint64 v) internal pure returns (bytes memory b) {
        b = new bytes(8);
        for (uint256 i; i < 8; ++i) {
            b[i] = bytes1(uint8(v >> (8 * i)));
        }
    }
}
