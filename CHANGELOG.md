# Changelog

## v0.1.0 (2026-10-29)

- The Zcash header relay, extracted from [Zecured](https://github.com/Himess/zecured) at `73de9f9` with its commit
  history. Its Solidity, tests, fixtures, tools, deploy script, parameters and feeder are unchanged.
- New: `IZcashHeaderRelayReader` (the read side, for any 0.8 compiler), the `ZcashFinalityGate` example, the ABI in
  `abi/`, docs/INTEGRATING.md and docs/RELAYING.md.
- New: a relay on Base Sepolia that follows Zcash Testnet from block 4,554,953
  (`contracts/deployments/seam-base-sepolia.relay.json`).
