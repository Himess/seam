#!/usr/bin/env bash
# Keeps a ZcashHeaderRelay in step with its Zcash network: Mainnet if the relay's networkId() is
# keccak256("zcash-mainnet"), Testnet if it is keccak256("zcash-testnet"); anything else, or a failed read,
# stops it. Reads the relay's best tip, fetches the headers that follow it from that network's public peers
# over the P2P protocol (read-only: contracts/test/relay/tools/zcash_p2p_headers.py) and submits them in
# batches. Submitting headers is permissionless: the relay checks every one itself (Equihash, difficulty,
# time, linkage).
#
#   scripts/relay-feed.sh <relay address> [--once]
#
# Environment, which the repository root's .env fills where it is unset: RELAY_RPC_URL (default
# http://127.0.0.1:8545, the local fork); the key, never printed: RELAYER_KEYSTORE with
# RELAYER_PASSWORD_FILE (an encrypted keystore, `cast wallet import`), or else RELAYER_PRIVATE_KEY (or
# DEPLOYER_PRIVATE_KEY), which cast takes only as an argument, where other users of the machine can read
# it; RELAY_BATCH (headers per transaction, 1 to 16, default 8).
#
# When the peers don't continue from the relay's tip (a Zcash reorganisation, or a branch nobody else
# mines), it asks them with a block locator of the relay's best chain, down to its checkpoint, and feeds
# their chain from where it leaves the relay's: the relay switches branches by work, at any depth. One
# line per round says what happened. A round that can't read the relay, or an hour without progress,
# ends the script, for the service manager to restart it where an operator sees it.
set -euo pipefail
# Bash 4.4 or later (macOS ships 3.2: `brew install bash`), and GNU timeout (`gtimeout` from Homebrew's coreutils).
if ((BASH_VERSINFO[0] < 4 || (BASH_VERSINFO[0] == 4 && BASH_VERSINFO[1] < 4))); then
  echo "relay-feed.sh needs bash 4.4 or later" >&2
  exit 2
fi
shopt -s inherit_errexit
TIMEOUT="$(command -v timeout || command -v gtimeout || true)"
[[ -n "$TIMEOUT" ]] || { echo "relay-feed.sh needs timeout (GNU coreutils; gtimeout on macOS)" >&2; exit 2; }

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FETCH="$ROOT/contracts/test/relay/tools/zcash_p2p_headers.py"
# .env fills only what the environment leaves unset, and exports nothing: the key never reaches a child process. One
# `NAME=value` a line (an `export ` before it is fine), the value in quotes or not, `~/` for the home directory; no
# comment after a value.
if [[ -f "$ROOT/.env" ]]; then
  while IFS= read -r line || [[ -n "$line" ]]; do
    [[ "$line" =~ ^(export[[:space:]]+)?([A-Z_][A-Z0-9_]*)=(.*)$ ]] || continue
    name="${BASH_REMATCH[2]}"
    value="${BASH_REMATCH[3]%$'\r'}"
    if [[ "$value" =~ ^\"(.*)\"$ || "$value" =~ ^\'(.*)\'$ ]]; then value="${BASH_REMATCH[1]}"; fi
    [[ "$value" == "~/"* ]] && value="$HOME/${value:2}"
    [[ -n "${!name:-}" || -z "$value" ]] || printf -v "$name" '%s' "$value"
  done < "$ROOT/.env"
fi

usage="usage: relay-feed.sh <relay address> [--once]"
RELAY="${1:?$usage}"
case "${2:-}" in
  "" | --once) ONCE="${2:-}" ;;
  *) echo "$usage" >&2; exit 2 ;;
esac
# cast reads the RPC URL from the environment, never from a command line, where other users of the machine could
# read it (and the key it may carry).
export ETH_RPC_URL="${RELAY_RPC_URL:-http://127.0.0.1:8545}"
if [[ -n "${RELAYER_KEYSTORE:-}" ]]; then
  SIGNER=(--keystore "$RELAYER_KEYSTORE" --password-file "${RELAYER_PASSWORD_FILE:?set RELAYER_PASSWORD_FILE too}")
  # Under a service manager the keystore must be where the service's user can read it, not in someone's home.
  [[ -r "$RELAYER_KEYSTORE" && -r "$RELAYER_PASSWORD_FILE" ]] ||
    { echo "can't read RELAYER_KEYSTORE or RELAYER_PASSWORD_FILE" >&2; exit 2; }
elif [[ -n "${RELAYER_PRIVATE_KEY:-}" ]]; then
  SIGNER=(--private-key "$RELAYER_PRIVATE_KEY")
else
  SIGNER=(--private-key "${DEPLOYER_PRIVATE_KEY:?set RELAYER_KEYSTORE, RELAYER_PRIVATE_KEY or DEPLOYER_PRIVATE_KEY}")
  echo "no relayer key: signing with DEPLOYER_PRIVATE_KEY" >&2
fi
if [[ "${SIGNER[0]}" == --private-key ]]; then
  echo "the key reaches cast on its command line, where other users of this machine can read it: RELAYER_KEYSTORE avoids that" >&2
fi
BATCH="${RELAY_BATCH:-8}"
# Batches above 16 grow costlier per header and run into argument and transaction gas limits.
[[ "$BATCH" =~ ^[0-9]+$ ]] && ((BATCH >= 1 && BATCH <= 16)) || { echo "RELAY_BATCH must be 1 to 16" >&2; exit 2; }
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

first_word() { awk 'NR==1 {print $1}'; }
display_order() {
  local h="${1#0x}" out="" i
  for ((i = ${#h} - 2; i >= 0; i -= 2)); do out+="${h:i:2}"; done
  echo "$out"
}
# cast's errors can name the RPC URL, which may carry a key: they reach the log with every URL replaced.
cast_() { cast "$@" 2> >(sed -E 's#[A-Za-z][A-Za-z0-9+.-]*://[^[:space:]"]+#<rpc url>#g' >&2); }
# A read of the relay, retried twice over a transient RPC error before it fails.
read_relay() {
  local attempt
  for attempt in 1 2 3; do
    cast_ call "$RELAY" "$@" && return 0
    ((attempt < 3)) && sleep 5
  done
  return 1
}
hash_at() { read_relay "bestBlockHashAt(uint256)(bytes32)" "$1"; }

nid="$(read_relay "networkId()(bytes32)")"
case "$nid" in
  "$(cast keccak zcash-mainnet)") NETWORK=mainnet ;;
  "$(cast keccak zcash-testnet)") NETWORK=testnet ;;
  *) echo "the relay's networkId() is neither Zcash network's" >&2; exit 1 ;;
esac
CHECKPOINT="$(read_relay "checkpointHeight()(uint256)" | first_word)"
echo "feeding a Zcash $NETWORK relay, $BATCH headers per transaction"

# The relay's best chain as a block locator: its tip, the nine blocks below, then twice as far back at each step,
# down to the checkpoint.
full_locator() {
  local h="$1" step=1 n=0 out="" hash
  while ((h > CHECKPOINT)); do
    hash="$(hash_at "$h")" # an assignment of its own: a failed read stops the script
    out+="${out:+,}$h:$(display_order "$hash")"
    n=$((n + 1))
    ((n < 10)) || step=$((step * 2))
    h=$((h - step))
  done
  hash="$(hash_at "$CHECKPOINT")"
  echo "$out${out:+,}$CHECKPOINT:$(display_order "$hash")"
}

# Fetches up to BATCH headers after the locator $1 into $TMP/headers.jsonl; returns the fetcher's status (3: no peer
# continues from the locator's blocks). Its log holds peer addresses: it stays in $TMP. The fetcher's own deadline,
# plus one peer's longest wait, ends before `timeout` would.
fetch() {
  local status=0
  rm -f "$TMP/headers.jsonl"
  "$TIMEOUT" 300 python3 -I "$FETCH" --network "$NETWORK" --locator "$1" --count "$BATCH" --deadline 180 \
    --out "$TMP/headers.jsonl" 2> "$TMP/fetch.log" || status=$?
  return "$status"
}

# The fetched headers the relay doesn't have yet, one raw header (0x hex) per line, into $TMP/new.txt (without the
# carriage returns Python writes on Windows).
unknown_headers() {
  python3 -I - "$TMP/headers.jsonl" "$RELAY" <<'EOF' | tr -d '\r' > "$TMP/new.txt"
import json, re, subprocess, sys, time
path, relay = sys.argv[1:3]
for line in open(path):
    item = json.loads(line)
    internal = "0x" + bytes.fromhex(item["hash"])[::-1].hex()
    # The RPC URL comes from the environment (ETH_RPC_URL): it is in neither this command nor the log. Retried twice
    # over a transient error, as every relay read is.
    for attempt in range(3):
        run = subprocess.run(
            ["cast", "call", relay, "headerOf(bytes32)((bool,bytes32,uint256,uint256,uint256,uint256,bytes32,bytes32))", internal],
            capture_output=True, text=True,
        )
        if run.returncode == 0:
            break
        time.sleep(5)
    if run.returncode != 0:
        sys.exit("reading the relay failed: " + re.sub(r"[A-Za-z][A-Za-z0-9+.-]*://\S+", "<rpc url>", run.stderr.strip())[:300])
    if not run.stdout.strip().startswith("(true"):
        print("0x" + item["raw"])
EOF
}

# Submits $TMP/new.txt: as one transaction, or, when the relay refuses the batch (one refused header reverts it
# all), one header at a time up to the first it refuses. Prints how many landed.
submit() {
  local landed=0 raw
  if cast_ send "${SIGNER[@]}" "$RELAY" "submitHeaders(bytes[])" "[$(paste -sd, "$TMP/new.txt")]" > /dev/null; then
    wc -l < "$TMP/new.txt" | tr -d ' '
    return 0
  fi
  while IFS= read -r raw; do
    cast_ send "${SIGNER[@]}" "$RELAY" "submitHeaders(bytes[])" "[$raw]" > /dev/null || break
    landed=$((landed + 1))
  done < "$TMP/new.txt"
  echo "$landed"
}

# The last header fetched, as height:hash, when the relay's best chain isn't the peers' yet (all of a batch was known
# already, or the batch just submitted didn't make its branch the best): the next round goes on from it.
cont=""
last_header() {
  tail -n 1 "$TMP/headers.jsonl" | python3 -I -c 'import json, sys; i = json.load(sys.stdin); print("{}:{}".format(i["height"], i["hash"]))' | tr -d '\r'
}
progress_at=$SECONDS
while true; do
  tip="$(read_relay "bestChainTip()(bytes32,uint256,uint256)")"
  tip_hash="$(sed -n 1p <<< "$tip")"
  tip_height="$(sed -n 2p <<< "$tip" | first_word)"
  status=0
  fetch "${cont:-$tip_height:$(display_order "$tip_hash")}" || status=$?
  if ((status == 3)); then
    # The peers don't continue from the relay's tip: find where their chain leaves the relay's.
    cont=""
    locator="$(full_locator "$tip_height")"
    status=0
    fetch "$locator" || status=$?
  fi

  count=0
  [[ -f "$TMP/headers.jsonl" ]] && count="$(wc -l < "$TMP/headers.jsonl" | tr -d ' ')"
  new=0
  landed=0
  if ((count > 0)); then
    unknown_headers
    new="$(wc -l < "$TMP/new.txt" | tr -d ' ')"
    if ((new > 0)); then
      landed="$(submit)"
      cont=""
      if ((landed == new)); then
        # A branch that isn't the best yet (a deep reorganisation): go on from its last header next round.
        last="$(last_header)"
        best="$(read_relay "bestChainTip()(bytes32,uint256,uint256)" | sed -n 1p)"
        [[ "$best" == "0x$(display_order "${last#*:}")" ]] || cont="$last"
      fi
    else
      # Known already, yet not the relay's best chain: go on from the last of them next round.
      cont="$(last_header)"
    fi
  fi

  case "$status" in
    0) ;;
    3) echo "no peer continues from the relay's best chain above its checkpoint" ;;
    *) echo "no peer answered in time" ;;
  esac
  ((landed == new)) || echo "the relay refused header $((landed + 1)) of $new new ones (its reason is above)"
  echo "relay tip $tip_height: fetched $count, new $new, submitted $landed"
  ((landed > 0 || (status == 0 && count == 0))) && progress_at=$SECONDS
  if ((SECONDS - progress_at > 3600)); then
    echo "an hour without progress: stopping, for the service manager to restart the feeder" >&2
    exit 1
  fi

  [[ -n "$ONCE" ]] && break
  # Caught up, or refused: wait. Known headers and full batches go on at once.
  ((count < BATCH && new == count || landed < new)) && sleep 25
done
