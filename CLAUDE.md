# Seam

A Zcash light client for EVM chains. Everything committed is in English.

## Do not trust your memory for Zcash APIs
Ironwood (NU6.3), NU7, v6 transactions and the current orchard, librustzcash, pczt and RISC Zero APIs may differ from
what you remember. Read the crate's source and CHANGELOG before using an API. Never invent APIs. If it doesn't compile
or a test fails, say so; never report unverified code as working.

## Rules
- The relay's Solidity (`contracts/src/relay/`) changes only with failing tests first, parity with the Python
  reference (`contracts/test/relay/tools/zcash_header_check.py`), and the Zebra source or ZIP each rule follows.
- Payments are Ironwood-only: no transparent, Sapling or Orchard-pool part.
- No Zcash txid, note commitment, nullifier, address or viewing key on an EVM chain, in an event, a log or a report.
  Recipients appear only as keccak256(salt32 || receiver43).
- Never log receivers, salts, keys, peer IPs, or RPC URLs that carry a key. Header fetches (`*.jsonl`,
  `*.jsonl.peers`) hold peer addresses and stay local.
- `TEST-ONLY_*` fixtures hold only disposable testnet wallets' data.
- Every mock is labelled MOCK in code and listed in the README.
- Always set `DEPLOYMENT_NAME` for `script/DeployRelay.s.sol`: the Testnet default, `base-sepolia`, is the record of
  Zecured's live run.
- forge and cast load the `.env` of the directory they run in: run them from this repository.

## Commands
contracts: cd contracts && forge build && forge test
feeder:    scripts/relay-feed.sh <relay address> [--once]   (reads .env at the repository root)
