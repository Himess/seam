#!/usr/bin/env python3
"""Fetch raw Zcash Testnet or Mainnet block headers from public peers over the Zcash P2P protocol.

Read-only: the script performs the version/verack handshake, sends `getheaders` and parses the
`headers` replies. It never relays anything. Standard library only.

Every header is checked before it is written:
  * the encoding is the Zcash v4 block header: 140 bytes, compactSize(1344) = fd 40 05, a 1344-byte
    Equihash (200, 9) solution, followed by a zero transaction count;
  * the block hash is recomputed here as SHA256d(header) (internal byte order);
  * each header's hashPrevBlock must equal the hash of the header before it (the first one must
    equal the locator hash that was asked for).

Usage:
  python3 -I zcash_p2p_headers.py --from-hash <display hash of block h-1> --from-height <h-1> \
      --count N --out headers.jsonl [--network mainnet] [--peer HOST:PORT ...]

Mainnet: on 2026-10-15 only Zakura peers answered `getheaders`; the Zebra peers closed the connection on
it, so the tool may go through many peers before one serves the headers.

Output: one JSON object per line: {"height", "hash" (display order), "raw" (hex, 1487 bytes)}.
"""

import argparse
import hashlib
import json
import random
import socket
import struct
import sys
import time

# Network magic, default port and DNS seeders, from Zebra v7.0.0-rc.0 (commit 6d1e414d6f55e4180d0e47baaa934bf97d5b4fec):
# zebra-chain/src/parameters/constants.rs (`magics`), zebra-chain/src/parameters/network.rs (`default_port`) and
# zebra-network/src/config.rs (`DEFAULT_MAINNET_PEERS`; for Testnet the two seeders this tool always used).
NETWORKS = {
    "testnet": {
        "magic": bytes.fromhex("fa1af9bf"),
        "port": 18233,
        "seeders": ["dnsseed.testnet.z.cash", "testnet.seeder.zfnd.org"],
        "protocol": 170180,
    },
    "mainnet": {
        "magic": bytes.fromhex("24e92764"),
        "port": 8233,
        "seeders": [
            "dnsseed.z.cash",
            "dnsseed.str4d.xyz",
            "mainnet.seeder.zfnd.org",
            "mainnet.seeder.shieldedinfra.net",
            "seeder.zec.rocks",
        ],
        "protocol": 170190,
    },
}
# Each network announces ZIP 259's MIN_NETWORK_PROTOCOL_VERSION for NU7 ("protocol" above): 170180 on Testnet,
# 170190 on Mainnet, where peers disconnect lower versions once NU7 activates (Zebra 7.0.0,
# zebra-network/src/protocol/external/types.rs `min_specified_for_upgrade`). Zebra 7.0.0 itself advertises
# CURRENT_NETWORK_PROTOCOL_VERSION = 170190 on both networks (zebra-network/src/constants.rs).
HEADER_PREFIX_LEN = 140
SOLUTION_LEN = 1344
HEADER_LEN = HEADER_PREFIX_LEN + 3 + SOLUTION_LEN  # 1487


def sha256d(data: bytes) -> bytes:
    return hashlib.sha256(hashlib.sha256(data).digest()).digest()


def varint(n: int) -> bytes:
    if n < 0xFD:
        return bytes([n])
    if n <= 0xFFFF:
        return b"\xfd" + struct.pack("<H", n)
    if n <= 0xFFFFFFFF:
        return b"\xfe" + struct.pack("<I", n)
    return b"\xff" + struct.pack("<Q", n)


def read_varint(buf: bytes, pos: int):
    first = buf[pos]
    if first < 0xFD:
        return first, pos + 1
    if first == 0xFD:
        return struct.unpack_from("<H", buf, pos + 1)[0], pos + 3
    if first == 0xFE:
        return struct.unpack_from("<I", buf, pos + 1)[0], pos + 5
    return struct.unpack_from("<Q", buf, pos + 1)[0], pos + 9


def message(magic: bytes, command: str, payload: bytes) -> bytes:
    cmd = command.encode("ascii").ljust(12, b"\x00")
    return magic + cmd + struct.pack("<I", len(payload)) + sha256d(payload)[:4] + payload


def net_addr(ip: str = "0.0.0.0", port: int = 0) -> bytes:
    ipv6 = b"\x00" * 10 + b"\xff\xff" + socket.inet_aton(ip)
    return struct.pack("<Q", 0) + ipv6 + struct.pack(">H", port)


class Peer:
    def __init__(self, host: str, port: int, magic: bytes, protocol: int, timeout: float = 20.0):
        self.host, self.port, self.magic, self.protocol = host, port, magic, protocol
        self.sock = socket.create_connection((host, port), timeout=timeout)
        self.sock.settimeout(timeout)
        self.buf = b""
        self.remote_version = None
        self.remote_agent = None

    def send(self, command: str, payload: bytes = b"") -> None:
        self.sock.sendall(message(self.magic, command, payload))

    def recv_message(self):
        while True:
            if len(self.buf) >= 24:
                if self.buf[:4] != self.magic:
                    raise RuntimeError("bad network magic from peer")
                length = struct.unpack_from("<I", self.buf, 16)[0]
                if length > 8_000_000:
                    raise RuntimeError("oversized message")
                if len(self.buf) >= 24 + length:
                    command = self.buf[4:16].rstrip(b"\x00").decode("ascii", "replace")
                    checksum = self.buf[20:24]
                    payload = self.buf[24 : 24 + length]
                    self.buf = self.buf[24 + length :]
                    if sha256d(payload)[:4] != checksum:
                        raise RuntimeError("bad message checksum")
                    return command, payload
            chunk = self.sock.recv(1 << 16)
            if not chunk:
                raise RuntimeError("peer closed the connection")
            self.buf += chunk

    def handshake(self) -> None:
        payload = (
            struct.pack("<iQq", self.protocol, 0, int(time.time()))
            + net_addr(self.host if self._is_ipv4(self.host) else "0.0.0.0", self.port)
            + net_addr()
            + struct.pack("<Q", random.getrandbits(64))
            + varint(len(b"/zecured-relay-fixtures:0.1/"))
            + b"/zecured-relay-fixtures:0.1/"
            + struct.pack("<i", 0)
            + b"\x00"  # relay = false
        )
        self.send("version", payload)
        got_version = got_verack = False
        while not (got_version and got_verack):
            command, body = self.recv_message()
            if command == "version":
                self.remote_version = struct.unpack_from("<i", body, 0)[0]
                agent_len, pos = read_varint(body, 80)
                self.remote_agent = body[pos : pos + agent_len].decode("ascii", "replace")
                got_version = True
                self.send("verack")
            elif command == "verack":
                got_verack = True
            elif command == "ping":
                self.send("pong", body)

    @staticmethod
    def _is_ipv4(host: str) -> bool:
        try:
            socket.inet_aton(host)
            return True
        except OSError:
            return False

    def get_headers(self, locator_internal: bytes):
        payload = struct.pack("<I", self.protocol) + varint(1) + locator_internal + b"\x00" * 32
        self.send("getheaders", payload)
        deadline = time.time() + 60
        while time.time() < deadline:
            command, body = self.recv_message()
            if command == "ping":
                self.send("pong", body)
            elif command == "headers":
                return parse_headers(body)
        raise RuntimeError("no headers reply")


def parse_headers(body: bytes):
    count, pos = read_varint(body, 0)
    out = []
    for _ in range(count):
        prefix = body[pos : pos + HEADER_PREFIX_LEN]
        sol_len, sol_pos = read_varint(body, pos + HEADER_PREFIX_LEN)
        if sol_len != SOLUTION_LEN or sol_pos - (pos + HEADER_PREFIX_LEN) != 3:
            raise RuntimeError("unexpected solution encoding")
        raw = body[pos : sol_pos + SOLUTION_LEN]
        if len(raw) != HEADER_LEN or len(prefix) != HEADER_PREFIX_LEN:
            raise RuntimeError("truncated header")
        pos = sol_pos + SOLUTION_LEN
        tx_count, pos = read_varint(body, pos)
        if tx_count != 0:
            raise RuntimeError("headers message with a non-zero transaction count")
        out.append(raw)
    if pos != len(body):
        raise RuntimeError("trailing bytes in headers message")
    return out


def candidate_peers(explicit, network):
    if explicit:
        for p in explicit:
            host, _, port = p.rpartition(":")
            yield host, int(port)
        return
    port = network["port"]
    addrs = []
    for seeder in network["seeders"]:
        try:
            for info in socket.getaddrinfo(seeder, port, socket.AF_INET, socket.SOCK_STREAM):
                addrs.append(info[4][0])
        except OSError:
            pass
    random.shuffle(addrs)
    seen = set()
    for a in addrs:
        if a not in seen:
            seen.add(a)
            yield a, port


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--from-hash", required=True, help="display-order hash of the block BEFORE the first one wanted")
    ap.add_argument("--from-height", required=True, type=int, help="height of that block")
    ap.add_argument("--count", required=True, type=int)
    ap.add_argument("--out", required=True)
    ap.add_argument("--network", choices=sorted(NETWORKS), default="testnet")
    ap.add_argument("--peer", action="append", help="HOST:PORT (repeatable); default: the network's DNS seeders")
    args = ap.parse_args()
    network = NETWORKS[args.network]

    locator = bytes.fromhex(args.from_hash)[::-1]
    height = args.from_height
    fetched = []
    peers = candidate_peers(args.peer, network)
    peer = None
    used = []
    while len(fetched) < args.count:
        if peer is None:
            try:
                host, port = next(peers)
            except StopIteration:
                print("ran out of peers", file=sys.stderr)
                return 1
            try:
                peer = Peer(host, port, network["magic"], network["protocol"])
                peer.handshake()
                described = f"{host}:{port} (protocol {peer.remote_version}, {peer.remote_agent})"
                print(f"connected to {described}", file=sys.stderr)
            except Exception as exc:  # noqa: BLE001 - try the next peer
                print(f"peer {host}:{port} failed: {exc}", file=sys.stderr)
                peer = None
                continue
        try:
            raws = peer.get_headers(locator)
        except Exception as exc:  # noqa: BLE001
            print(f"getheaders failed on {peer.host}: {exc}", file=sys.stderr)
            peer = None
            continue
        if not raws:
            print("peer returned no headers (tip reached?)", file=sys.stderr)
            break
        if raws[0][4:36] != locator:
            # The peer did not continue from our locator (e.g. it does not know that block yet).
            print(
                f"peer {peer.host} answered from prev={raws[0][4:36][::-1].hex()} instead of "
                f"{locator[::-1].hex()}; trying another peer",
                file=sys.stderr,
            )
            peer = None
            continue
        if described not in used:
            used.append(described)  # only peers whose headers were kept
        for raw in raws:
            if raw[4:36] != locator:
                raise SystemExit(f"linkage error at height {height + 1}")
            block_hash = sha256d(raw)
            height += 1
            fetched.append({"height": height, "hash": block_hash[::-1].hex(), "raw": raw.hex()})
            locator = block_hash
            if len(fetched) >= args.count:
                break
        print(f"{len(fetched)}/{args.count} headers (last height {height})", file=sys.stderr)

    with open(args.out, "w") as f:
        for item in fetched:
            f.write(json.dumps(item) + "\n")
    with open(args.out + ".peers", "w") as f:
        f.write("\n".join(used) + "\n")
    print(f"wrote {len(fetched)} headers to {args.out}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
