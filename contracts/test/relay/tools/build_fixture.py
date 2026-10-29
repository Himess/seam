#!/usr/bin/env python3
"""Build a ZcashHeaderRelay test fixture from headers fetched with zcash_p2p_headers.py.

  python3 -I build_fixture.py --headers all.jsonl --checkpoint-height H --count N \
      --description "..." --peers all.jsonl.peers --out ../fixtures/testnet_x.json [--cipherscan] \
      [--network mainnet] [--zebra-checkpoints main-checkpoints.txt]

The fixture holds:
  * the trusted checkpoint (block H): its hash and the 113 (= 102 + 11) previous-block contexts the
    relay needs for its first difficulty adjustment: heights H-112 .. H with time and nBits;
  * the N raw 1487-byte headers H+1 .. H+N, with their SHA256d hashes.

Hashes are given in internal byte order (`*_hashes`, the bytes32 the relay uses) and in display order
(`*_hashes_display`, as block explorers show them).

With --cipherscan every context entry and header is compared against the CipherScan API of the
network (https://api.testnet.cipherscan.app/api/block/<height>, https://api.mainnet.cipherscan.app/...),
an indexer independent of the P2P peers: hash, previous hash, time and nBits for every block, plus
version, merkle root, nonce and solution for the full headers. Any mismatch aborts.

With --zebra-checkpoints (Zebra's checkpoint list, e.g. zebra-chain/src/parameters/checkpoint/
main-checkpoints.txt at a release tag) every fetched hash at a listed height must match, and the
selection must lie between two listed heights: the hashPrevBlock linkage then pins every header in
between to Zebra's list. Any mismatch aborts.
"""

import argparse
import datetime
import hashlib
import json
import os
import struct
import sys
import time
import urllib.request

CONTEXT = 102 + 11  # PoWAveragingWindow (NU7) + PoWMedianBlockSpan
NETWORKS = {
    "testnet": {"name": "Zcash Testnet", "cipherscan": "https://api.testnet.cipherscan.app/api/block/{}",
                "nu7_activation_height": 4465026},
    # Mainnet NU7 height: ZIP 259 and Zebra 7.0.0 (zebra-chain/src/parameters/constants.rs).
    "mainnet": {"name": "Zcash Mainnet", "cipherscan": "https://api.mainnet.cipherscan.app/api/block/{}",
                "nu7_activation_height": 3543405},
}


def fetch_cipherscan(url: str, height: int):
    req = urllib.request.Request(url.format(height), headers={"User-Agent": "zecured-relay-fixtures"})
    for attempt in range(5):
        try:
            with urllib.request.urlopen(req, timeout=30) as resp:
                return json.load(resp)
        except Exception as exc:  # noqa: BLE001
            if attempt == 4:
                raise
            time.sleep(1 + attempt)
    return None


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--headers", required=True)
    ap.add_argument("--peers", required=True)
    ap.add_argument("--checkpoint-height", required=True, type=int)
    ap.add_argument("--count", required=True, type=int)
    ap.add_argument("--description", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--cipherscan", action="store_true")
    ap.add_argument("--network", choices=sorted(NETWORKS), default="testnet")
    ap.add_argument("--zebra-checkpoints", help="Zebra checkpoint list (<height> <display hash> per line)")
    ap.add_argument("--zebra-ref", default="Zebra v7.0.0-rc.0, commit 6d1e414d6f55e4180d0e47baaa934bf97d5b4fec",
                    help="the Zebra release the checkpoint list was taken from (recorded in the fixture)")
    args = ap.parse_args()
    net = NETWORKS[args.network]

    by_height = {}
    for line in open(args.headers):
        item = json.loads(line)
        by_height[item["height"]] = item
    cp = args.checkpoint_height
    ctx_heights = list(range(cp - CONTEXT + 1, cp + 1))
    run_heights = list(range(cp + 1, cp + 1 + args.count))
    for h in ctx_heights + run_heights:
        if h not in by_height:
            raise SystemExit(f"height {h} not in {args.headers}")

    def fields(item):
        raw = bytes.fromhex(item["raw"])
        return {
            "version": struct.unpack_from("<i", raw, 0)[0],
            "prev_display": raw[4:36][::-1].hex(),
            "merkle_display": raw[36:68][::-1].hex(),
            "time": struct.unpack_from("<I", raw, 100)[0],
            "bits": struct.unpack_from("<I", raw, 104)[0],
            "nonce": raw[108:140],
            "solution": raw[143:].hex(),
        }

    # Linkage across the whole selection (the fetcher already checked it; re-check here).
    sel = ctx_heights + run_heights
    for a, b in zip(sel, sel[1:]):
        if fields(by_height[b])["prev_display"] != by_height[a]["hash"]:
            raise SystemExit(f"linkage broken between {a} and {b}")

    cross = "not performed"
    if args.cipherscan:
        for h in sel:
            ours, f = by_height[h], fields(by_height[h])
            cs = fetch_cipherscan(net["cipherscan"], h)
            checks = {
                "height": int(cs["height"]) == h,
                "hash": cs["hash"] == ours["hash"],
                "prev": cs["previous_block_hash"] == f["prev_display"],
                "time": int(cs["timestamp"]) == f["time"],
                "bits": int(cs["bits"], 16) == f["bits"],
            }
            if h in run_heights:
                checks["version"] = int(cs["version"]) == f["version"]
                checks["merkle"] = cs["merkle_root"] == f["merkle_display"]
                checks["nonce"] = cs["nonce"] in (f["nonce"].hex(), f["nonce"][::-1].hex())
                checks["solution"] = cs["solution"] == f["solution"]
            bad = [k for k, ok in checks.items() if not ok]
            if bad:
                raise SystemExit(f"CipherScan mismatch at {h}: {bad}")
            time.sleep(0.05)
        cross = (
            f"every entry compared with CipherScan {net['name'].split()[-1]} API "
            f"({net['cipherscan'].format('<height>')}) on {datetime.date.today().isoformat()}: hash, previous "
            "hash, time, nBits; for the full headers also version, merkle root, nonce and solution. All matched."
        )

    pinned = None
    if args.zebra_checkpoints:
        listed = {}
        for line in open(args.zebra_checkpoints):
            if line.strip():
                height, display = line.split()
                listed[int(height)] = display
        known = {h: item["hash"] for h, item in by_height.items()}
        first = min(by_height)
        known[first - 1] = fields(by_height[first])["prev_display"]  # the fetcher's locator
        matched = sorted(h for h in known if h in listed)
        for h in matched:
            if listed[h] != known[h]:
                raise SystemExit(f"Zebra checkpoint mismatch at {h}")
        below = [h for h in matched if h < sel[0]]
        above = [h for h in matched if h >= sel[-1]]
        if not below or not above:
            raise SystemExit("the selection is not enclosed by two Zebra checkpoints")
        lo, hi = below[-1], above[0]
        for a in range(lo, hi):
            raw = bytes.fromhex(by_height[a + 1]["raw"])
            if hashlib.sha256(hashlib.sha256(raw).digest()).digest()[::-1].hex() != known[a + 1]:
                raise SystemExit(f"hash mismatch at {a + 1}")
            if fields(by_height[a + 1])["prev_display"] != known[a]:
                raise SystemExit(f"linkage broken between {a} and {a + 1}")
        pinned = (
            f"heights {lo + 1} .. {hi} link the checkpoints at {lo} ({listed[lo]}) and {hi} ({listed[hi]}) in "
            f"{os.path.basename(args.zebra_checkpoints)} of {args.zebra_ref} "
            "(zebra-chain/src/parameters/checkpoint/): every header of this fixture is pinned by that list"
        )

    # Record the peers' software, not their IP addresses.
    peers = sorted({line.strip().split(" ", 1)[1] for line in open(args.peers) if " " in line.strip()})
    cp_item = by_height[cp]
    kind = net["name"].split()[-1]
    source = {
        "headers": f"Zcash {kind} P2P network, `getheaders` (protocol 170180) via "
        f"contracts/test/relay/tools/zcash_p2p_headers.py from public {kind} peer(s) found via the DNS seeders: "
        + "; ".join(peers),
        "fetched": datetime.date.today().isoformat(),
        "verification": "SHA256d recomputed for every header and hashPrevBlock linkage checked; "
        "tools/zcash_header_check.py re-checks Equihash, PoW, nBits retarget and time rules offline",
        "cross_check": cross,
    }
    if pinned:
        source["zebra_checkpoints"] = pinned
    fixture = {
        "description": args.description,
        "network": net["name"],
        "source": source,
        "nu7_activation_height": net["nu7_activation_height"],
        "checkpoint_height": cp,
        "checkpoint_hash": "0x" + bytes.fromhex(cp_item["hash"])[::-1].hex(),
        "checkpoint_hash_display": cp_item["hash"],
        "context_heights": ctx_heights,
        "context_times": [fields(by_height[h])["time"] for h in ctx_heights],
        "context_bits": [fields(by_height[h])["bits"] for h in ctx_heights],
        "context_hashes_display": [by_height[h]["hash"] for h in ctx_heights],
        "header_heights": run_heights,
        "header_hashes": ["0x" + bytes.fromhex(by_height[h]["hash"])[::-1].hex() for h in run_heights],
        "header_hashes_display": [by_height[h]["hash"] for h in run_heights],
        "header_raws": ["0x" + by_height[h]["raw"] for h in run_heights],
    }
    with open(args.out, "w") as f:
        json.dump(fixture, f, indent=1)
        f.write("\n")
    print(f"wrote {args.out}: checkpoint {cp}, {len(ctx_heights)} context entries, {len(run_heights)} headers")
    return 0


if __name__ == "__main__":
    sys.exit(main())
