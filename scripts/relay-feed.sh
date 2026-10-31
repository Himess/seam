#!/usr/bin/env bash
# Keeps a ZcashHeaderRelay in step with its Zcash network: Mainnet if the relay's networkId() is
# keccak256("zcash-mainnet"), Testnet otherwise. Reads the relay's best tip, fetches the headers that
# follow it from that network's public peers over the P2P protocol (read-only:
# contracts/test/relay/tools/zcash_p2p_headers.py) and submits them in batches. Submitting headers
# is permissionless: the relay checks every one itself (Equihash, difficulty, time, linkage).
#
#   scripts/relay-feed.sh <relay address> [--once]
#
# Environment: RELAY_RPC_URL (default http://127.0.0.1:8545, the local fork); the key, never printed:
# RELAYER_KEYSTORE with RELAYER_PASSWORD_FILE (an encrypted keystore, `cast wallet import`), or else
# RELAYER_PRIVATE_KEY (default: DEPLOYER_PRIVATE_KEY from the repo-root .env), which cast takes only as an
# argument, where other users of the machine can read it; RELAY_BATCH (headers per transaction, default
# 20). If the peers no longer continue from the relay's tip (a Zcash reorg), the feeder restarts from up
# to 20 blocks below it; the relay switches branches by work.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FETCH="$ROOT/contracts/test/relay/tools/zcash_p2p_headers.py"
if [[ -f "$ROOT/.env" ]]; then
  set -a
  # shellcheck disable=SC1091
  source "$ROOT/.env"
  set +a
fi

RELAY="${1:?usage: relay-feed.sh <relay address> [--once]}"
ONCE="${2:-}"
RPC="${RELAY_RPC_URL:-http://127.0.0.1:8545}"
if [[ -n "${RELAYER_KEYSTORE:-}" ]]; then
  SIGNER=(--keystore "$RELAYER_KEYSTORE" --password-file "${RELAYER_PASSWORD_FILE:?set RELAYER_PASSWORD_FILE too}")
else
  SIGNER=(--private-key "${RELAYER_PRIVATE_KEY:-${DEPLOYER_PRIVATE_KEY:?set RELAYER_KEYSTORE, RELAYER_PRIVATE_KEY or DEPLOYER_PRIVATE_KEY}}")
  echo "the key reaches cast on its command line, where other users of this machine can read it: RELAYER_KEYSTORE avoids that" >&2
fi
BATCH="${RELAY_BATCH:-20}"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

first_word() { awk 'NR==1 {print $1}'; }
display_order() { python3 -I -c 'import sys; print(bytes.fromhex(sys.argv[1][2:])[::-1].hex())' "$1"; }
# cast's errors can name the RPC URL, which may carry a key: they reach the log with every URL replaced.
cast_() { cast "$@" 2> >(sed -E 's#[A-Za-z][A-Za-z0-9+.-]*://[^[:space:]"]+#<rpc url>#g' >&2); }

NETWORK=testnet
if [[ "$(cast_ call --rpc-url "$RPC" "$RELAY" "networkId()(bytes32)")" == "$(cast keccak zcash-mainnet)" ]]; then
  NETWORK=mainnet
fi

back=0
while true; do
  tip_height="$(cast_ call --rpc-url "$RPC" "$RELAY" "bestChainTip()(bytes32,uint256,uint256)" | sed -n 2p | first_word)"
  from_height=$((tip_height - back))
  from_hash="$(cast_ call --rpc-url "$RPC" "$RELAY" "bestBlockHashAt(uint256)(bytes32)" "$from_height")"
  rm -f "$TMP/headers.jsonl"
  python3 -I "$FETCH" --network "$NETWORK" --from-hash "$(display_order "$from_hash")" --from-height "$from_height" \
    --count "$BATCH" --out "$TMP/headers.jsonl" 2> "$TMP/fetch.log" || true
  count=0
  [[ -f "$TMP/headers.jsonl" ]] && count="$(wc -l < "$TMP/headers.jsonl" | tr -d ' ')"

  if [[ "$count" -gt 0 ]]; then
    # Skip headers the relay already has (after stepping back below its tip).
    headers="$(python3 -I - "$TMP/headers.jsonl" "$RPC" "$RELAY" <<'EOF'
import json, re, subprocess, sys
path, rpc, relay = sys.argv[1:4]
raws = []
for line in open(path):
    item = json.loads(line)
    internal = "0x" + bytes.fromhex(item["hash"])[::-1].hex()
    run = subprocess.run(
        ["cast", "call", "--rpc-url", rpc, relay, "headerOf(bytes32)((bool,bytes32,uint256,uint256,uint256,uint256,bytes32,bytes32))", internal],
        capture_output=True, text=True,
    )
    if run.returncode != 0:
        # Neither the command (it holds the RPC URL) nor a URL in cast's error reaches the log.
        sys.exit("reading the relay failed: " + re.sub(r"[A-Za-z][A-Za-z0-9+.-]*://\S+", "<rpc url>", run.stderr.strip())[:300])
    if not run.stdout.strip().startswith("(true"):
        raws.append("0x" + item["raw"])
print("[" + ",".join(raws) + "]" if raws else "")
EOF
)"
    if [[ -n "$headers" ]]; then
      cast_ send --rpc-url "$RPC" "${SIGNER[@]}" "$RELAY" "submitHeaders(bytes[])" "$headers" > /dev/null
    fi
    back=0
    echo "relay tip: $(cast_ call --rpc-url "$RPC" "$RELAY" "bestChainTip()(bytes32,uint256,uint256)" | sed -n 2p | first_word)"
  elif grep -q "instead of" "$TMP/fetch.log" && [[ "$back" -lt 20 ]]; then
    back=$((back + 5)) # the peers left the relay's tip: retry from further back
    continue
  fi

  [[ -n "$ONCE" ]] && break
  [[ "$count" -lt "$BATCH" ]] && sleep 25
done
