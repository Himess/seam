# Relaying

A relay knows only the headers someone submits.
- **Who can feed it:** anyone. The relay checks every header, so a feeder can delay a relay but can't make it accept a
  bad header.
- **Several feeders:** `submitHeaders` skips headers the relay already has, so two feeders racing waste gas but nothing
  fails. Better: one feeds, and the other takes over when the tip stalls.
- **Your own:** an app whose users wait on a relay (a deadline, a pending settlement) should run a feeder of its own,
  or at least watch the tip and start one when it stalls, rather than count on someone else's.

## Feed a relay

You need:
- Foundry's `cast`;
- bash 4.4 or later and GNU `timeout` (macOS ships bash 3.2 and no `timeout`: `brew install bash coreutils`);
- Python 3 (standard library only);
- outbound TCP to Zcash peers (port 18233 on Testnet, 8233 on Mainnet);
- a Base Sepolia RPC URL;
- a key that holds a little Base Sepolia ETH.

```sh
cp .env.example .env && chmod 600 .env          # set RELAY_RPC_URL and the key (RELAYER_KEYSTORE is best)
scripts/relay-feed.sh <relay address> --once    # one batch
scripts/relay-feed.sh <relay address>           # keeps following; waits 25 s when caught up
```

How the feeder works:
- **Each batch:** it reads the relay's tip and fetches the next headers over the P2P protocol from public peers of the
  relay's network: Mainnet if `networkId()` is `keccak256("zcash-mainnet")`, Testnet if `keccak256("zcash-testnet")`;
  anything else, or a read that fails, stops it. The fetch is read-only:
  [`zcash_p2p_headers.py`](../contracts/test/relay/tools/zcash_p2p_headers.py), with every wait on a peer bounded. It
  then drops the headers the relay already has and submits the rest in one transaction. If the relay refuses one
  header, the batch reverts before it is sent; the feeder then submits one header at a time, up to the refused one.
- **On a Zcash reorganisation:** if the peers no longer continue from the relay's tip, it asks them with a block
  locator of the relay's best chain (its tip, the nine blocks below, then twice as far back at each step, down to the
  checkpoint) and feeds their chain from where it leaves the relay's, until the relay switches branches by work: at
  any depth.
- **Output:** one line per round: the relay's tip, the headers fetched, new and submitted. Peer addresses stay in a
  temporary directory, and every URL in an error is replaced, since an RPC URL can carry a key; cast reads the RPC URL
  from the environment, never from a command line. An hour without progress stops the feeder, so that the service
  manager restarts it in the open.
- **Batch size:** `RELAY_BATCH`, 1 to 16, default 8. Batches of 4 to 8 headers are the cheapest per header: 597,000
  gas at 4 and 645,000 at 16, where memory grows
  ([Zecured's MAINNET.md](https://github.com/Himess/zecured/blob/73de9f909e270e8aea10c2b221905f62a6a4c4df/docs/MAINNET.md#3-a-mainnet-deployment)).
  Once the relay has caught up, each batch holds the headers that arrived since the last one.
- **Settings:** the repository root's `.env` fills what the environment leaves unset, and exports nothing.

## Cost

Measured on Seam's relays on 29 October 2026, from the feeders' receipts on Base Sepolia (`gasUsed`,
`effectiveGasPrice`, `l1Fee`) at a gas price of 0.005 to 0.006 gwei. The L1 data fee was under 0.01 % of each fee.

| | Headers | Gas per header | ETH per header |
|---|---|---|---|
| Catching up, batches of 8 (the last one of 5) | 557, in 70 transactions | 604,805 | 0.00000341 |
| Following the tip, 1 to 8 headers a batch | 15, in 4 transactions | 600,388 | 0.00000335 |
| Mainnet, catching up before NU7 (17-block window) | 315, in 40 transactions | 578,105 | 0.00000321 |

At NU7's 25-second blocks that is 3,456 headers and about **0.012 ETH a day**: 0.1 Base Sepolia ETH feeds a relay for
about eight days. Mainnet before NU7 has 75-second blocks: 1,152 headers and about 0.0037 ETH a day.

## Keep it running

- **Linux (systemd):** the unit that feeds Seam's relay, with placeholder paths. Create the user first
  (`useradd --system --home-dir /var/lib/seam --no-create-home --shell /usr/sbin/nologin seam`), and make `.env`
  readable by it only (`chown seam:seam .env && chmod 600 .env`). Save the unit as
  `/etc/systemd/system/seam-feeder.service`, then run `systemctl enable --now seam-feeder`.
- **macOS (launchd):** save the template below as `~/Library/LaunchAgents/<label>.plist`. Use absolute paths: launchd
  doesn't expand `~`. Its `PATH` must reach `cast`, `python3` and Homebrew's bash and `gtimeout` (`/opt/homebrew/bin`
  on Apple silicon, `/usr/local/bin` on Intel).

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
    <string>/Users/YOU/seam/scripts/relay-feed.sh</string>
    <string>0xRELAY</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key>
    <string>/Users/YOU/.foundry/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin</string>
  </dict>
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

Either way the feeder exits when a read of the relay fails three times in a row, or after an hour without progress,
and the service restarts it after a minute.

## Deploy your own relay

1. **Fetch headers** around the block you'll start from:
   `zcash_p2p_headers.py --from-hash … --from-height … --count …`.
2. **Build the checkpoint** a few hundred blocks below the tip, cross-checked with an independent indexer:
   `build_fixture.py … --checkpoint-height <H> --count 1 --cipherscan`, then `zcash_header_check.py fixture …`.
3. **Deploy.** From `contracts/`, with `DEPLOYER_PRIVATE_KEY` set:
   `RELAY_CHECKPOINT=<file> DEPLOYMENT_NAME=<name> forge script script/DeployRelay.s.sol --rpc-url … --broadcast`.
   - `DEPLOYMENT_NAME` names the record, `deployments/<name>.relay.json`. Only a broadcast writes it, and the script
     refuses a name whose record exists.
4. **Verify** the source on the chain's explorer (`--verify` with an Etherscan API key, or `--verifier sourcify`).
5. **Feed** it to the tip, and compare its tip with an explorer.

**Mainnet:** `config/relay-mainnet.json` sets NU7's height to 3,543,405 (ZIP 259, Zebra 7.0.0). A relay deployed with
`NEVER` instead refuses the first NU7 block and can't go past it.

How Seam's own relay was deployed:
[the commit that added its record](https://github.com/Himess/seam/commit/97aa01bf2ebc9909f9636da5ebb52c91d1c28731).
