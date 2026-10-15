#!/usr/bin/env python3
"""Independent reference checks for Zcash Testnet and Mainnet block headers (standard library only).

This is a second, independent implementation of everything ZcashHeaderRelay.sol verifies. It is used
to cross-check the fixtures and to compare against the Solidity code. It follows:

  * Equihash: librustzcash `components/equihash/src/verify.rs` (`is_valid_solution`,
    recursive `tree_validator`), the code Zebra calls through `Solution::check`;
  * block hash / PoW: SHA256d of the 1487-byte header, compared as a little-endian integer with the
    target decoded from nBits (zcashd `CheckProofOfWork`, Zebra `difficulty_is_valid`);
  * difficulty and time rules: Zebra v7.0.0-rc.0 (commit 6d1e414d6f55e4180d0e47baaa934bf97d5b4fec)
    `zebra-state/src/service/check/difficulty.rs`, `zebra-state/src/service/check.rs`,
    `zebra-chain/src/parameters/network_upgrade.rs` and `network.rs` (ZIP 218: 25 s spacing and a
    102-block averaging window from NU7; the Testnet minimum-difficulty gap is 450 s before and after
    NU7 and does not exist on Mainnet; the 90-minute maximum time applies on Mainnet at every height).
    The parameters of both networks, with sources, are in docs/ref/FACTS.md.

CLI:
  python3 -I zcash_header_check.py chain headers.jsonl [--network mainnet] [--equihash]  # JSON lines
  python3 -I zcash_header_check.py fixture fixture.json   # a relay test fixture (network from the file)
"""

import argparse
import hashlib
import json
import struct
import sys

# --- Network-independent parameters ------------------------------------------------------------
BLOCK_MAX_TIME_SINCE_MEDIAN = 90 * 60
POW_MEDIAN_BLOCK_SPAN = 11
POW_DAMPING_FACTOR = 4
POW_MAX_ADJUST_UP_PERCENT = 16
POW_MAX_ADJUST_DOWN_PERCENT = 32
HEADER_LEN = 1487


def set_compact(bits: int):
    """zcashd arith_uint256::SetCompact. Returns (value, negative, overflow)."""
    size = bits >> 24
    word = bits & 0x007FFFFF
    if size <= 3:
        value = word >> (8 * (3 - size))
    else:
        value = word << (8 * (size - 3))
    negative = word != 0 and (bits & 0x00800000) != 0
    overflow = word != 0 and (size > 34 or (word > 0xFF and size > 33) or (word > 0xFFFF and size > 32))
    return value % (1 << 256), negative, overflow


def get_compact(value: int) -> int:
    """zcashd arith_uint256::GetCompact (non-negative values)."""
    size = (value.bit_length() + 7) // 8
    if size <= 3:
        compact = (value << (8 * (3 - size))) & 0xFFFFFFFF
    else:
        compact = (value >> (8 * (size - 3))) & 0xFFFFFFFF
    if compact & 0x00800000:
        compact >>= 8
        size += 1
    assert compact & ~0x007FFFFF == 0
    return compact | (size << 24)


def to_target(bits: int):
    """Zebra CompactDifficulty::to_expanded: None for negative, zero or overflowing values."""
    value, negative, overflow = set_compact(bits)
    if negative or overflow or value == 0:
        return None
    return value


# --- Network parameters (Zebra v7.0.0-rc.0, ZIP 218, ZIP 259; docs/ref/FACTS.md) --------------------
class Network:
    def __init__(self, name, pow_limit_exact, min_difficulty_start, max_time_start, nu7):
        self.name = name
        # PoWLimit rounded through its compact form, as zcashd and Zebra use it
        self.pow_limit = to_target(get_compact(pow_limit_exact))
        self.pow_limit_bits = get_compact(self.pow_limit)
        self.min_difficulty_start = min_difficulty_start  # None: no minimum-difficulty rule
        self.max_time_start = max_time_start  # first height of the 90-minute rule
        self.nu7 = nu7  # None: not activated (height unknown)


TESTNET = Network("Zcash Testnet", (1 << 251) - 1, min_difficulty_start=299_188, max_time_start=653_606,
                  nu7=4_465_026)
MAINNET = Network("Zcash Mainnet", (1 << 243) - 1, min_difficulty_start=None, max_time_start=0, nu7=None)
NETWORKS = {"testnet": TESTNET, "mainnet": MAINNET}
NU7_ACTIVATION_TESTNET = TESTNET.nu7
POW_LIMIT = TESTNET.pow_limit  # 0x07ffff << 232 (kept for make_synthetic.py)
POW_LIMIT_BITS = TESTNET.pow_limit_bits  # 0x2007ffff


def work(bits: int) -> int:
    target = to_target(bits)
    return ((~target & ((1 << 256) - 1)) // (target + 1)) + 1


def sha256d(data: bytes) -> bytes:
    return hashlib.sha256(hashlib.sha256(data).digest()).digest()


# --- Header parsing ----------------------------------------------------------------------------
class Header:
    def __init__(self, raw: bytes):
        if len(raw) != HEADER_LEN or raw[140:143] != b"\xfd\x40\x05":
            raise ValueError("not a 1487-byte Zcash header with a 1344-byte solution")
        self.raw = raw
        self.version = struct.unpack_from("<i", raw, 0)[0]
        self.prev = raw[4:36]
        self.merkle_root = raw[36:68]
        self.block_commitments = raw[68:100]
        self.time = struct.unpack_from("<I", raw, 100)[0]
        self.bits = struct.unpack_from("<I", raw, 104)[0]
        self.nonce = raw[108:140]
        self.solution = raw[143:]
        self.hash = sha256d(raw)


# --- Equihash (n = 200, k = 9) -----------------------------------------------------------------
N, K = 200, 9
COLLISION_BITS = N // (K + 1)  # 20
INDEX_BITS = COLLISION_BITS + 1  # 21


class EquihashError(Exception):
    pass


def indices_from_minimal(solution: bytes):
    if len(solution) != (1 << K) * INDEX_BITS // 8:
        raise EquihashError("InvalidParams")
    acc = int.from_bytes(solution, "big")
    total = len(solution) * 8
    return [(acc >> (total - INDEX_BITS * (j + 1))) & ((1 << INDEX_BITS) - 1) for j in range(1 << K)]


def equihash_state(header_prefix_140: bytes):
    person = b"ZcashPoW" + struct.pack("<II", N, K)
    state = hashlib.blake2b(digest_size=(512 // N) * N // 8, person=person)
    state.update(header_prefix_140)
    return state


def leaf_chunks(state, index: int):
    s = state.copy()
    s.update(struct.pack("<I", index // 2))
    digest = s.digest()
    start = (index % 2) * (N // 8)
    value = int.from_bytes(digest[start : start + N // 8], "big")
    # expand_array(.., 20, 0): ten 20-bit chunks, most significant first
    return [(value >> (N - COLLISION_BITS * (c + 1))) & ((1 << COLLISION_BITS) - 1) for c in range(K + 1)]


def equihash_check(header_prefix_140: bytes, solution: bytes) -> None:
    """Raises EquihashError(kind) exactly like the reference's recursive validator."""
    indices = indices_from_minimal(solution)
    state = equihash_state(header_prefix_140)

    def tree(lo, hi):
        if hi - lo == 1:
            return leaf_chunks(state, indices[lo]), [indices[lo]]
        mid = (lo + hi) // 2
        a_hash, a_idx = tree(lo, mid)
        b_hash, b_idx = tree(mid, hi)
        if a_hash[0] != b_hash[0]:
            raise EquihashError("Collision")
        if b_idx[0] < a_idx[0]:
            raise EquihashError("OutOfOrder")
        if set(a_idx) & set(b_idx):
            raise EquihashError("DuplicateIdxs")
        return [x ^ y for x, y in zip(a_hash[1:], b_hash[1:])], a_idx + b_idx

    root_hash, _ = tree(0, 1 << K)
    if root_hash[0] != 0:
        raise EquihashError("NonZeroRootHash")


# --- Difficulty adjustment (Zebra v7.0.0-rc.0) ------------------------------------------------
def is_nu7(height: int, net: Network = TESTNET) -> bool:
    return net.nu7 is not None and height >= net.nu7


def averaging_window(height: int, net: Network = TESTNET) -> int:
    return 102 if is_nu7(height, net) else 17


def target_spacing(height: int, net: Network = TESTNET) -> int:
    return 25 if is_nu7(height, net) else 75


def min_difficulty_gap(height: int, net: Network = TESTNET):
    if net.min_difficulty_start is None or height < net.min_difficulty_start:
        return None
    multiplier = 18 if is_nu7(height, net) else 6
    return target_spacing(height, net) * multiplier


def median(values):
    s = sorted(values)
    return s[len(s) // 2]


def trunc_div(a: int, b: int) -> int:
    q = abs(a) // abs(b)
    return q if (a >= 0) == (b > 0) else -q


def expected_bits(height: int, time: int, prev_bits, prev_times, net: Network = TESTNET) -> int:
    """prev_bits / prev_times: the previous blocks' nBits and times, newest first (index 0 = parent)."""
    gap = min_difficulty_gap(height, net)
    if gap is not None and time - prev_times[0] > gap:
        return net.pow_limit_bits
    w = averaging_window(height, net)
    awt = w * target_spacing(height, net)
    targets = [to_target(b) for b in prev_bits[:w]]
    quotients = sum(t // w for t in targets)
    remainders = sum(t % w for t in targets)
    mean = quotients + remainders // w
    newer = median(prev_times[:POW_MEDIAN_BLOCK_SPAN])
    older = median(prev_times[w : w + POW_MEDIAN_BLOCK_SPAN])
    damped = awt + trunc_div(newer - older - awt, POW_DAMPING_FACTOR)
    lo = awt * (100 - POW_MAX_ADJUST_UP_PERCENT) // 100
    hi = awt * (100 + POW_MAX_ADJUST_DOWN_PERCENT) // 100
    bounded = max(lo, min(hi, damped))
    threshold = min(net.pow_limit, (mean // awt) * bounded)
    return get_compact(threshold)


def check_time(height: int, time: int, prev_times, net: Network = TESTNET):
    mtp = median(prev_times[:POW_MEDIAN_BLOCK_SPAN])
    if time <= mtp:
        return "TimeTooEarly"
    if height >= net.max_time_start and time > mtp + BLOCK_MAX_TIME_SINCE_MEDIAN:
        return "TimeTooLate"
    return None


def check_header(height: int, header: Header, prev_bits, prev_times, equihash: bool = True, net: Network = TESTNET):
    """Returns a list of problems (empty when the header passes every check)."""
    problems = []
    if header.version < 4:
        problems.append("version")
    t = check_time(height, header.time, prev_times, net)
    if t:
        problems.append(t)
    exp = expected_bits(height, header.time, prev_bits, prev_times, net)
    if exp != header.bits:
        problems.append(f"bits {header.bits:08x} != expected {exp:08x}")
    target = to_target(header.bits)
    if target is None or target > net.pow_limit:
        problems.append("invalid target")
    elif int.from_bytes(header.hash, "little") > target:
        problems.append("hash above target")
    if equihash:
        try:
            equihash_check(header.raw[:140], header.solution)
        except EquihashError as e:
            problems.append(f"equihash {e}")
    return problems


# --- CLI -----------------------------------------------------------------------------------------
def check_chain(path: str, equihash: bool, net: Network = TESTNET) -> int:
    items = [json.loads(line) for line in open(path)]
    headers = [Header(bytes.fromhex(it["raw"])) for it in items]
    bad = 0
    min_diff = 0
    span = 102 + POW_MEDIAN_BLOCK_SPAN
    for i, (it, h) in enumerate(zip(items, headers)):
        if h.hash[::-1].hex() != it["hash"]:
            print(f"{it['height']}: hash mismatch")
            bad += 1
        if i > 0 and h.prev != headers[i - 1].hash:
            print(f"{it['height']}: linkage")
            bad += 1
        height = it["height"]
        need = averaging_window(height, net) + POW_MEDIAN_BLOCK_SPAN
        if i < need:
            continue
        prev = headers[max(0, i - span) : i][::-1]
        problems = check_header(height, h, [p.bits for p in prev], [p.time for p in prev], equihash, net)
        if h.bits == net.pow_limit_bits:
            min_diff += 1
        if problems:
            bad += 1
            print(f"{height}: {problems}")
    checked = len(items) - min(len(items), 17 + POW_MEDIAN_BLOCK_SPAN)
    print(f"{net.name}: checked {len(items)} headers ({items[0]['height']}..{items[-1]['height']}), "
          f"{min_diff} minimum-difficulty blocks, {bad} problems")
    return 1 if bad else 0


def check_fixture(path: str) -> int:
    fx = json.load(open(path))
    net = {n.name: n for n in NETWORKS.values()}[fx.get("network", TESTNET.name)]
    ctx_bits = [int(b) for b in fx["context_bits"]]
    ctx_times = list(fx["context_times"])
    ctx_heights = list(fx["context_heights"])
    assert ctx_heights == list(range(ctx_heights[0], ctx_heights[0] + len(ctx_heights)))
    cp_hash = bytes.fromhex(fx["checkpoint_hash"][2:])
    assert ctx_heights[-1] == fx["checkpoint_height"]
    bits, times = ctx_bits[:], ctx_times[:]
    prev_hash = cp_hash
    height = fx["checkpoint_height"]
    bad = 0
    for raw_hex, hash_hex in zip(fx["header_raws"], fx["header_hashes"]):
        height += 1
        h = Header(bytes.fromhex(raw_hex[2:]))
        if h.hash != bytes.fromhex(hash_hex[2:]):
            print(f"{height}: hash mismatch")
            bad += 1
        if h.prev != prev_hash:
            print(f"{height}: linkage")
            bad += 1
        problems = check_header(height, h, bits[::-1], times[::-1], equihash=True, net=net)
        if problems:
            print(f"{height}: {problems}")
            bad += 1
        bits.append(h.bits)
        times.append(h.time)
        prev_hash = h.hash
    print(f"{path} ({net.name}): {len(fx['header_raws'])} headers after checkpoint {fx['checkpoint_height']}, "
          f"{bad} problems")
    return 1 if bad else 0


def main() -> int:
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    c = sub.add_parser("chain")
    c.add_argument("path")
    c.add_argument("--network", choices=sorted(NETWORKS), default="testnet")
    c.add_argument("--equihash", action="store_true")
    f = sub.add_parser("fixture")
    f.add_argument("path")
    args = ap.parse_args()
    if args.cmd == "chain":
        return check_chain(args.path, args.equihash, NETWORKS[args.network])
    return check_fixture(args.path)


if __name__ == "__main__":
    sys.exit(main())
