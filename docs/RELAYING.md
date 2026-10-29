# Relaying

A relay knows only the headers someone submits.
- **Who can feed it:** anyone. The relay checks every header, so a feeder can delay a relay but can't make it accept a
  bad header.
- **Several feeders:** `submitHeaders` skips headers the relay already has, so two feeders racing waste gas but nothing
  fails. Better: one feeds, and the other takes over when the tip stalls.

## Feed a relay

You need:
- Foundry's `cast`;
- Python 3 (standard library only);
- outbound TCP to Zcash Testnet peers (port 18233);
- a Base Sepolia RPC URL;
- a key that holds a little Base Sepolia ETH.

```sh
cp .env.example .env && chmod 600 .env          # set RELAY_RPC_URL and RELAYER_PRIVATE_KEY
scripts/relay-feed.sh <relay address> --once    # one batch
scripts/relay-feed.sh <relay address>           # keeps following; waits 25 s when caught up
```

How the feeder works:
- **Each batch:** it reads the relay's tip and fetches the next headers from public Zcash peers over the P2P protocol.
  The fetch is read-only: [`zcash_p2p_headers.py`](../contracts/test/relay/tools/zcash_p2p_headers.py). It then drops
  the headers the relay already has and submits the rest in one transaction.
- **On a Zcash reorganisation:** if the peers no longer continue from the relay's tip, it steps back up to 20 blocks;
  the relay switches branches by work.
- **Output:** it prints the relay's tip after each batch. Peer addresses stay in a temporary directory.
- **Batch size:** use `RELAY_BATCH=8`. Batches of 4 to 8 headers are the cheapest per header: 597,000 gas at 4 and
  645,000 at 16, where memory grows
  ([Zecured's MAINNET.md](https://github.com/Himess/zecured/blob/73de9f909e270e8aea10c2b221905f62a6a4c4df/docs/MAINNET.md#3-a-mainnet-deployment)).
  Once the relay has caught up, each batch holds the headers that arrived since the last one.

## Cost

Measured on Seam's relay on 29 October 2026, from the feeder's receipts on Base Sepolia (`gasUsed`,
`effectiveGasPrice`, `l1Fee`) at a gas price of 0.005 to 0.006 gwei. The L1 data fee was under 0.01 % of each fee.

| | Headers | Gas per header | ETH per header |
|---|---|---|---|
| Catching up, batches of 8 (the last one of 5) | 557, in 70 transactions | 604,805 | 0.00000341 |
| Following the tip, 1 to 8 headers a batch | 15, in 4 transactions | 600,388 | 0.00000335 |

At NU7's 25-second blocks that is 3,456 headers and about **0.012 ETH a day**: 0.1 Base Sepolia ETH feeds a relay for
about eight days.

## Keep it running

- **Linux (systemd):** the unit that feeds Seam's relay, with placeholder paths. Create the user first
  (`useradd --system --home-dir /var/lib/seam --no-create-home --shell /usr/sbin/nologin seam`), and make `.env`
  readable by it only (`chown seam:seam .env && chmod 600 .env`). Save the unit as
  `/etc/systemd/system/seam-feeder.service`, then run `systemctl enable --now seam-feeder`.
- **macOS (launchd):** save the template below as `~/Library/LaunchAgents/<label>.plist`. Use absolute paths: launchd
  doesn't expand `~`. `bash -lc` loads the login profile, so `cast` is on the PATH.

```ini
[Unit]
Description=Seam relay feeder
After=network-online.target
Wants=network-online.target

[Service]
User=seam
Group=seam
WorkingDirectory=/opt/seam
# cast and python3 must be on this PATH.
Environment=PATH=/opt/foundry/bin:/usr/local/bin:/usr/bin:/bin
Environment=HOME=/var/lib/seam
ExecStart=/opt/seam/scripts/relay-feed.sh 0xRELAY
Restart=always
RestartSec=60
StateDirectory=seam
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true

[Install]
WantedBy=multi-user.target
```

```sh
journalctl -u seam-feeder -f            # the relay's tip after each batch
systemctl stop seam-feeder              # stop it; `systemctl disable seam-feeder` keeps it stopped after a reboot
```

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>com.example.seam-feeder</string>
  <key>ProgramArguments</key>
  <array>
    <string>/bin/bash</string>
    <string>-lc</string>
    <string>exec /Users/YOU/seam/scripts/relay-feed.sh 0xRELAY</string>
  </array>
  <key>WorkingDirectory</key>
  <string>/Users/YOU/seam</string>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>ThrottleInterval</key>
  <integer>60</integer>
  <key>StandardOutPath</key>
  <string>/Users/YOU/Library/Logs/seam-feeder.log</string>
  <key>StandardErrorPath</key>
  <string>/Users/YOU/Library/Logs/seam-feeder.log</string>
</dict>
</plist>
```

```sh
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.example.seam-feeder.plist
launchctl print gui/$(id -u)/com.example.seam-feeder | grep -E 'state|pid'
launchctl bootout gui/$(id -u)/com.example.seam-feeder    # stop it
```

Either way the feeder exits on an RPC error, and the service restarts it after a minute.

## Deploy your own relay

1. **Fetch headers** around the block you'll start from:
   `zcash_p2p_headers.py --from-hash … --from-height … --count …`.
2. **Build the checkpoint** a few hundred blocks below the tip, cross-checked with an independent indexer:
   `build_fixture.py … --checkpoint-height <H> --count 1 --cipherscan`, then `zcash_header_check.py fixture …`.
3. **Deploy.** From `contracts/`, with `DEPLOYER_PRIVATE_KEY` set:
   `RELAY_CHECKPOINT=<file> DEPLOYMENT_NAME=<name> forge script script/DeployRelay.s.sol --rpc-url … --broadcast`.
   - Always set `DEPLOYMENT_NAME`: the Testnet default, `base-sepolia`, is the record of Zecured's live run.
   - A simulation writes the record too, with a simulated address.
4. **Verify** the source on the chain's explorer (`--verify` with an Etherscan API key, or `--verifier sourcify`).
5. **Feed** it to the tip, and compare its tip with an explorer.

**Mainnet:** `config/relay-mainnet.json` keeps NU7's height at `NEVER` until a release sets it, with tests. A relay
deployed with `NEVER` refuses the first NU7 block and can't go past it.

How Seam's own relay was deployed:
[the commit that added its record](https://github.com/Himess/seam/commit/97aa01bf2ebc9909f9636da5ebb52c91d1c28731).
