// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

/// @title ZcashEquihash
/// @notice Equihash (n = 200, k = 9) solution check for a Zcash block header, using the EIP-152 BLAKE2b
///         compression precompile (address 0x09).
/// @dev Reference followed: librustzcash `components/equihash/src/verify.rs` (`is_valid_solution`, recursive
///      `tree_validator`), equihash crate 0.3.0, which Zebra calls through `Solution::check`
///      (zebra-chain/src/work/equihash.rs, Zebra v7.0.0-rc.0). zcashd's former `equihash.cpp` `IsValidSolution`
///      implements the same rules.
///
///      Parameters (Zcash Protocol Specification § 7.6.1 "Equihash"):
///        - collision length 20 bits = n / (k + 1); 2^k = 512 indices of 21 bits each, packed big-endian into the
///          1344-byte minimal solution (`indices_from_minimal`);
///        - BLAKE2b with a 50-byte digest and personalization "ZcashPoW" || le32(200) || le32(9). The hash input is
///          the 140-byte header prefix (version, prevHash, merkleRoot, hashBlockCommitments, time, nBits, 32-byte
///          nonce) followed by le32(i / 2); each digest holds two 25-byte (200-bit) strings, and index i takes
///          bytes [(i % 2) * 25, (i % 2) * 25 + 25).
///
///      BLAKE2b: the 144-byte message is two 128-byte blocks. The first block (header bytes 0..127, counter 128,
///      not final) is the same for every index, so it is compressed once. Each index then needs only its final
///      block: header bytes 128..139 || le32(i / 2) || zero padding, counter 144, final flag set.
///
///      Tree check. The reference keeps, per node, the XOR of its leaves as 20-bit chunks ("expanded" to 3 bytes)
///      and trims the leading chunk at each level. Here a node is one word: (first index << 224) | S, where S is
///      the 200-bit XOR of all leaf strings below the node, read as a big-endian integer, so chunk c of S is
///      bits [20c, 20c + 20) from the top. This only re-lays out the same bits:
///        - "has_collision(a, b, 3)" at level r (1..9) compares chunk r - 1 of both children. Chunks 0..r-2 of
///          both children are already zero, so it is equivalent to `S_parent >> (200 - 20 r) == 0`;
///        - "indices_before": the right child's first index must not be smaller than the left child's
///          (OutOfOrder). Equal first indices are duplicates and are reported as DuplicateIdxs;
///        - "distinct_indices" at every merge holds for all merges if and only if all 512 indices are distinct,
///          which is checked once, with a hash set, while the indices are decoded;
///        - the root must be zero: after nine levels its only non-trimmed chunk is chunk 9, the others are zero.
///      Validity is therefore identical to the reference. The reported error kind is the reference's for any
///      solution with a single defect; with several defects the first one found may differ (the reference walks
///      the tree depth first, this code checks duplicates first and then level by level).
library ZcashEquihash {
    /// @notice Outcome of a check. `InvalidEncoding` covers a header that is not 1487 bytes with compactSize(1344).
    enum Result {
        Valid,
        InvalidEncoding,
        DuplicateIdxs,
        Collision,
        OutOfOrder,
        NonZeroRootHash
    }

    /// @notice The BLAKE2b compression precompile is missing or failed.
    error Blake2bPrecompileFailed();

    uint256 internal constant HEADER_LENGTH = 1487;
    uint256 internal constant INDEX_COUNT = 512;

    /// @dev BLAKE2b state after parameter-block initialisation (RFC 7693 § 2.5): h[i] = IV[i] XOR word i of the
    ///      parameter block {digest length 50, key length 0, fanout 1, depth 1, salt 0, personalization
    ///      "ZcashPoW" || le32(200) || le32(9)}, as 8 little-endian 64-bit words (the EIP-152 input encoding).
    ///      Recomputed from the IV in test/relay/ZcashEquihash.t.sol.
    bytes32 internal constant H_INIT_0 = 0x3ac9bdf267e6096a3ba7ca8485ae67bb2bf894fe72f36e3cf1361d5f3af54fa5;
    bytes32 internal constant H_INIT_1 = 0xd182e6ad7f520e511f6c3e2b8c68059b31de2088c389ec48b1217e1310cde05b;

    /// @notice Checks the Equihash solution of a serialized Zcash block header.
    /// @param header 1487 bytes: 140-byte prefix || compactSize(1344) = fd 40 05 || 1344-byte solution.
    function verify(bytes memory header) internal view returns (Result) {
        if (header.length != HEADER_LENGTH) return Result.InvalidEncoding;
        if (uint8(header[140]) != 0xfd || uint8(header[141]) != 0x40 || uint8(header[142]) != 0x05) {
            return Result.InvalidEncoding;
        }
        uint256[] memory nodes = new uint256[](INDEX_COUNT);
        if (!leafNodes(header, nodes)) return Result.DuplicateIdxs;
        return checkTree(nodes);
    }

    /// @notice Decodes the 512 indices, rejects duplicates and computes every leaf string.
    /// @dev Writes nodes[j] = (index_j << 224) | leaf(index_j). Returns false on a duplicate index (nodes is then
    ///      only partially written). `header` must be 1487 bytes (checked by `verify`).
    function leafNodes(bytes memory header, uint256[] memory nodes) internal view returns (bool distinct) {
        bool precompileOk = true;
        distinct = true;
        assembly ("memory-safe") {
            let hdr := add(header, 32)
            let buf := mload(0x40) // precompile input (213 bytes, 256 reserved), then its output at buf + 256
            let table := add(buf, 320) // hash set: 1024 words, entry = index + 1
            mstore(0x40, add(table, 32768))
            calldatacopy(table, calldatasize(), 32768) // zero the hash set

            // First block: rounds = 12 (big-endian u32), h = initial state, m = header[0..128),
            // t = 128 (little-endian u128), f = 0.
            mstore(buf, shl(224, 12))
            mstore(add(buf, 4), H_INIT_0)
            mstore(add(buf, 36), H_INIT_1)
            mstore(add(buf, 68), mload(hdr))
            mstore(add(buf, 100), mload(add(hdr, 32)))
            mstore(add(buf, 132), mload(add(hdr, 64)))
            mstore(add(buf, 164), mload(add(hdr, 96)))
            mstore(add(buf, 196), shl(248, 128))
            // A missing precompile would "succeed" with no output, hence the return size check. (Two statements:
            // Yul evaluates arguments right to left, so returndatasize() must be read after the call.)
            precompileOk := staticcall(gas(), 0x09, buf, 213, add(buf, 256), 64)
            precompileOk := and(precompileOk, eq(returndatasize(), 64))

            // Final-block template: h = state after the first block, m = header[128..140) || le32(g) || 0,
            // t = 144, f = 1.
            mstore(add(buf, 4), mload(add(buf, 256)))
            mstore(add(buf, 36), mload(add(buf, 288)))
            mstore(add(buf, 68), and(mload(add(hdr, 128)), not(shr(96, not(0)))))
            mstore(add(buf, 100), 0)
            mstore(add(buf, 132), 0)
            mstore(add(buf, 164), 0)
            mstore(add(buf, 196), shl(248, 144))
            mstore8(add(buf, 212), 1)

            let dst := add(nodes, 32)
            // Index j is bits [21 j, 21 j + 21) of the solution, most significant bit first. Eight indices fill
            // exactly 21 bytes, so the solution is read as 64 groups: index m of a group is the word at the group
            // start shifted right by 235 - 21 m. `hdr` becomes the end of the solution.
            let grp := add(hdr, 143)
            hdr := add(hdr, 1487)
            for {} lt(grp, hdr) { grp := add(grp, 21) } {
                let word := mload(grp)
                for { let shift := 235 } gt(shift, 87) { shift := sub(shift, 21) } {
                    let idx := and(shr(shift, word), 0x1fffff)

                    // Duplicate check: open addressing over 1024 slots keyed by the low 10 bits, linear probing.
                    let p := add(table, shl(5, and(idx, 1023)))
                    for {} 1 {} {
                        let v := mload(p)
                        if iszero(v) {
                            mstore(p, add(idx, 1))
                            break
                        }
                        if eq(v, add(idx, 1)) {
                            distinct := 0
                            break
                        }
                        p := add(p, 32)
                        if eq(p, add(table, 32768)) { p := table }
                    }
                    if iszero(distinct) { break }

                    // Final BLAKE2b block for g = idx / 2: m[12..16) = le32(g) (its last byte stays 0, g < 2^20).
                    mstore8(add(buf, 80), shr(1, idx))
                    mstore8(add(buf, 81), shr(9, idx))
                    mstore8(add(buf, 82), shr(17, idx))
                    if iszero(staticcall(gas(), 0x09, buf, 213, add(buf, 256), 64)) {
                        precompileOk := 0
                        break
                    }
                    // Leaf = digest bytes [(idx % 2) * 25, +25) as a 200-bit big-endian integer.
                    mstore(dst, or(shl(224, idx), shr(56, mload(add(add(buf, 256), mul(and(idx, 1), 25))))))
                    dst := add(dst, 32)
                }
                if iszero(and(distinct, precompileOk)) { break }
            }
        }
        if (!precompileOk) revert Blake2bPrecompileFailed();
    }

    /// @notice Checks the k = 9 tree over 512 leaf nodes, in place.
    /// @dev nodes[j] = (index << 224) | 200-bit leaf string. Indices must already be known to be distinct.
    ///      Level r merges pairs (2j, 2j + 1) into j: S = S_left XOR S_right must have its top 20 r bits zero
    ///      (Collision), the right first index must not be below the left one (OutOfOrder), and after nine levels
    ///      S must be zero (NonZeroRootHash).
    function checkTree(uint256[] memory nodes) internal pure returns (Result result) {
        if (nodes.length != INDEX_COUNT) return Result.InvalidEncoding;
        assembly ("memory-safe") {
            let base := add(nodes, 32)
            let mask200 := sub(shl(200, 1), 1)
            let indexMask := not(mask200)
            let end := add(base, 16384) // the current level's nodes are [base, end)
            // Level r = 1 .. 9 checks chunk r - 1: shift = 200 - 20 r = 180, 160, .., 20. Parents are written in
            // place: parent j goes to slot j after children 2j and 2j + 1 were read.
            for { let shift := 180 } shift { shift := sub(shift, 20) } {
                let w := base
                for { let pa := base } lt(pa, end) { pa := add(pa, 64) } {
                    let a := mload(pa)
                    let b := mload(add(pa, 32))
                    let x := and(xor(a, b), mask200)
                    let collision := shr(shift, x)
                    // The first index sits in the top bits and the indices are distinct, so comparing the whole
                    // words compares the first indices.
                    if or(collision, lt(b, a)) {
                        // OutOfOrder (4), unless the collision check, which the reference runs first, fails (3).
                        result := 4
                        if collision { result := 3 }
                        break
                    }
                    mstore(w, or(and(a, indexMask), x))
                    w := add(w, 32)
                }
                if result { break }
                end := w
            }
            // NonZeroRootHash
            if iszero(result) { if and(mload(base), mask200) { result := 5 } }
        }
    }
}
