#!/usr/bin/env python3
"""Build contracts/test/relay/fixtures/testnet_synthetic.json (SYNTHETIC test data).

Two kinds of data, both derived from the real fixture testnet_recent.json:

1. `mutations`: copies of the real header 4,488,000 with one targeted defect each (bit flips in the
   solution, nonce and merkle root; two swapped leaves; swapped subtrees; a duplicated half). The
   expected Equihash error kind of each one is computed here with the reference port in
   zcash_header_check.py.

2. `forks`: synthetic headers on top of the real block 4,488,002 (R3). They are NOT real Zcash
   blocks, but their Equihash solutions are genuine: they are found by Tromp's solver (equihash
   0.3.0, `solver` feature) in research/relay-synthetic-headers at the Testnet minimum difficulty
   (nBits 0x2007ffff), which the Testnet rule allows when a block's time is more than 450 s after
   its parent's. Every synthetic header is re-verified here (Equihash, PoW, expected nBits).

Usage: python3 -I make_synthetic.py <solver-binary> <fixtures-dir>
"""

import hashlib
import importlib.util
import json
import os
import struct
import subprocess
import sys

sys.dont_write_bytecode = True  # importing the checker below must not leave __pycache__ in the repository
HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("chk", os.path.join(HERE, "zcash_header_check.py"))
chk = importlib.util.module_from_spec(spec)
spec.loader.exec_module(chk)

MIN_BITS = 0x2007FFFF


def le_hex(b: bytes) -> str:
    return "0x" + b.hex()


def encode_indices(indices):
    acc = 0
    for v in indices:
        acc = (acc << 21) | v
    return acc.to_bytes(1344, "big")


def equihash_kind(raw: bytes) -> str:
    try:
        chk.equihash_check(raw[:140], raw[143:])
        return "Valid"
    except chk.EquihashError as e:
        return str(e)


def mutations(raw: bytes):
    out = {}

    def flip(label, byte_index, bit):
        m = bytearray(raw)
        m[byte_index] ^= 1 << bit
        out[label] = bytes(m)

    flip("solution_bit_flip", 143 + 700, 3)
    flip("nonce_bit_flip", 108 + 5, 0)
    flip("merkle_root_bit_flip", 36 + 7, 6)
    idx = chk.indices_from_minimal(raw[143:])
    swapped = idx[:]
    swapped[0], swapped[1] = swapped[1], swapped[0]
    out["swapped_leaves"] = raw[:143] + encode_indices(swapped)
    out["swapped_subtrees"] = raw[:143] + encode_indices(idx[256:] + idx[:256])
    out["duplicated_half"] = raw[:143] + encode_indices(idx[:256] + idx[:256])
    return out


def solve(solver, prefix108: bytes, bits: int, want: str, tag: bytes):
    res = subprocess.run(
        [solver, prefix108.hex(), f"{bits:08x}", want, tag.hex(), "4000"],
        capture_output=True, text=True, check=True,
    )
    nonce_hex, solution_hex, hash_display = res.stdout.split()
    return bytes.fromhex(nonce_hex), bytes.fromhex(solution_hex), hash_display, res.stderr.strip()


def build_header(version, prev, merkle, commitments, time, bits, nonce, solution):
    prefix = struct.pack("<i", version) + prev + merkle + commitments + struct.pack("<II", time, bits)
    assert len(prefix) == 108
    return prefix + nonce + b"\xfd\x40\x05" + solution


def main() -> int:
    solver, fixtures = sys.argv[1], sys.argv[2]
    recent = json.load(open(os.path.join(fixtures, "testnet_recent.json")))
    raws = [bytes.fromhex(r[2:]) for r in recent["header_raws"]]
    heights = recent["header_heights"]
    # Chain data (oldest first) for the expected-nBits model: context + real headers.
    bits_chain = list(recent["context_bits"]) + [chk.Header(r).bits for r in raws]
    times_chain = list(recent["context_times"]) + [chk.Header(r).time for r in raws]

    # --- 1. mutations of the real header R1 = 4,488,000 ---
    r1 = raws[0]
    muts = mutations(r1)
    mutation_entries = {}
    for label, raw in muts.items():
        mutation_entries[label] = {"raw": le_hex(raw), "expected_equihash_error": equihash_kind(raw)}
        print(f"mutation {label}: {mutation_entries[label]['expected_equihash_error']}")

    # Leaf hashes of R1 for the BLAKE2b unit test: (index, 25-byte string) for its first 4 indices.
    state = chk.equihash_state(r1[:140])
    leaf_vectors = []
    for i in chk.indices_from_minimal(r1[143:])[:4]:
        s = state.copy()
        s.update(struct.pack("<I", i // 2))
        d = s.digest()
        start = (i % 2) * 25
        leaf_vectors.append({"index": i, "leaf": "0x" + d[start:start + 25].hex()})

    # --- 2. synthetic forks on top of the real block R3 = 4,488,002 ---
    r3 = chk.Header(raws[2])
    r3_height = heights[2]
    ctx_len = len(recent["context_bits"])
    # ancestry of R3 (newest first) for the model
    upto = ctx_len + 3
    base_bits = bits_chain[:upto][::-1]
    base_times = times_chain[:upto][::-1]

    # label, parent label, time delta from parent, nBits, want PoW pass/fail
    # Branch A (7 blocks, 451 s apart) and branch B (8 blocks, 600 s then 500 s apart) both fork from R3.
    # Different gaps make the 11-block medians on the two branches differ, so a test can tell which
    # branch's history the relay used.
    plan = [(f"A{i}", "R3" if i == 1 else f"A{i - 1}", 451, MIN_BITS, "pass") for i in range(1, 8)]
    plan += [(f"B{i}", "R3" if i == 1 else f"B{i - 1}", 600 if i == 1 else 500, MIN_BITS, "pass") for i in range(1, 9)]
    plan += [
        ("D1", "R3", 30, MIN_BITS, "pass"),    # min-difficulty nBits without the 450 s gap
        ("E1", "R3", 450, MIN_BITS, "pass"),   # boundary: a 450 s gap does not qualify
        ("F1", "R3", 451, MIN_BITS, "fail"),   # valid Equihash, hash above the target
    ]
    known = {"R3": (r3.hash, r3_height, base_bits, base_times)}
    forks = {}
    for label, parent, delta, bits, want in plan:
        p_hash, p_height, p_bits, p_times = known[parent]
        time = p_times[0] + delta
        merkle = hashlib.sha256(f"zecured relay synthetic merkle {label}".encode()).digest()
        commitments = hashlib.sha256(f"zecured relay synthetic commitments {label}".encode()).digest()
        prefix = struct.pack("<i", 4) + p_hash + merkle + commitments + struct.pack("<II", time, bits)
        tag = f"zecured-{label}".encode()
        nonce, solution, hash_display, log = solve(solver, prefix, bits, want, tag)
        raw = build_header(4, p_hash, merkle, commitments, time, bits, nonce, solution)
        h = chk.Header(raw)
        assert h.hash[::-1].hex() == hash_display
        assert equihash_kind(raw) == "Valid", label
        target = chk.to_target(bits)
        passes = int.from_bytes(h.hash, "little") <= target
        assert passes == (want == "pass"), label
        height = p_height + 1
        expected = chk.expected_bits(height, time, p_bits, p_times)
        problems = chk.check_header(height, h, p_bits, p_times, equihash=False)
        forks[label] = {
            "parent": parent,
            "height": height,
            "median_time_past": chk.median(p_times[:11]),
            "time": time,
            "bits": bits,
            "expected_bits": expected,
            "pow_valid": passes,
            "consensus_problems": problems,
            "hash": le_hex(h.hash),
            "hash_display": h.hash[::-1].hex(),
            "raw": le_hex(raw),
        }
        print(f"fork {label}: height {height} dt={delta} expected_bits={expected:08x} problems={problems} ({log})")
        known[label] = (h.hash, height, [bits] + p_bits, [time] + p_times)

    fixture = {
        "description": "SYNTHETIC relay test data derived from the real Zcash Testnet fixture testnet_recent.json. "
        "Not real Zcash blocks.",
        "mutations_of": {"height": heights[0], "hash": recent["header_hashes"][0]},
        "mutations_note": "Real header 4,488,000 with one defect each; expected_equihash_error from the "
        "reference port (librustzcash equihash verify.rs) in tools/zcash_header_check.py",
        "mutations": mutation_entries,
        "leaf_vectors_note": "BLAKE2b-400 ('ZcashPoW', n=200, k=9) leaf strings of header 4,488,000 for its "
        "first four solution indices, computed with Python hashlib",
        "leaf_vectors": leaf_vectors,
        "forks_note": "Synthetic headers on top of the real block 4,488,002. Equihash solutions found by "
        "Tromp's solver (equihash 0.3.0) via research/relay-synthetic-headers at the Testnet minimum "
        "difficulty; merkle roots and block commitments are arbitrary labelled values.",
        "fork_base": {"height": r3_height, "hash": le_hex(r3.hash), "time": r3.time},
        "forks": forks,
    }
    out = os.path.join(fixtures, "testnet_synthetic.json")
    with open(out, "w") as f:
        json.dump(fixture, f, indent=1)
        f.write("\n")
    print(f"wrote {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
