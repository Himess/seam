// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;

import {Test} from "forge-std/Test.sol";
import {console} from "forge-std/console.sol";
import {ZcashEquihash} from "../../src/relay/ZcashEquihash.sol";
import {EquihashHarness} from "./ZcashEquihash.t.sol";

/// @notice RQ-10 / K4: the relay's Equihash check agrees with the reference Zebra calls (the `equihash` crate 0.3.0)
///         on 592 cases built from the 74 real Testnet headers of the relay fixtures: the real solutions, and each
///         changed seven ways (a solution bit, two indices swapped, an index duplicated, an index plus one, two
///         subtrees swapped at a random level, a nonce bit, a header prefix bit). The corpus and its labels come
///         from signer/crates/deposit-witness/src/bin/equihash_corpus.rs (fixed seed).
contract ZcashEquihashCorpusTest is Test {
    string internal constant CORPUS = "test/relay/fixtures/equihash_corpus.json";
    EquihashHarness internal harness;

    function setUp() public {
        harness = new EquihashHarness();
    }

    function _name(ZcashEquihash.Result r) internal pure returns (string memory) {
        if (r == ZcashEquihash.Result.Valid) return "Valid";
        if (r == ZcashEquihash.Result.InvalidEncoding) return "InvalidEncoding";
        if (r == ZcashEquihash.Result.DuplicateIdxs) return "DuplicateIdxs";
        if (r == ZcashEquihash.Result.Collision) return "Collision";
        if (r == ZcashEquihash.Result.OutOfOrder) return "OutOfOrder";
        return "NonZeroRootHash";
    }

    /// Validity always agrees. The kind of defect agrees too, except where the library documents that it may
    /// not: a duplicated index also changes that leaf's hash, two defects, and the library checks duplicates
    /// first while the reference walks the tree depth first (src/relay/ZcashEquihash.sol, header comment).
    function test_agreesWithTheReferenceOnEveryCase() public view {
        string memory json = vm.readFile(CORPUS);
        bytes[] memory raws = vm.parseJsonBytesArray(json, ".raws");
        string[] memory kinds = vm.parseJsonStringArray(json, ".kinds");
        string[] memory mutations = vm.parseJsonStringArray(json, ".mutations");
        assertEq(raws.length, 592);
        assertEq(kinds.length, raws.length);
        uint256 valid;
        uint256 documentedDifferences;
        for (uint256 i; i < raws.length; ++i) {
            string memory got = _name(harness.verify(raws[i]));
            bool referenceValid = keccak256(bytes(kinds[i])) == keccak256("Valid");
            assertEq(keccak256(bytes(got)) == keccak256("Valid"), referenceValid, mutations[i]);
            if (referenceValid) ++valid;
            if (keccak256(bytes(got)) != keccak256(bytes(kinds[i]))) {
                // The only kind difference allowed: the library's duplicate check comes first.
                assertEq(mutations[i], "an index duplicated", "unexpected kind difference");
                assertEq(got, "DuplicateIdxs");
                ++documentedDifferences;
            }
        }
        assertEq(valid, 74, "the real solutions");
        console.log("cases:", raws.length, "valid:", valid);
        console.log("kind differences (a duplicated index, checked first here):", documentedDifferences);
    }
}
