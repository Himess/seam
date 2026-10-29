# Changelog

## v0.2.0 (unreleased)

- `config/relay-mainnet.json` sets NU7's Mainnet height to 3,543,405 (ZIP 259, Zebra 7.0.0) instead of `NEVER`, so a
  Mainnet relay deployed from it follows Mainnet across NU7. The test parameters, the Python reference and
  `build_fixture.py` use the same height.
- `zcash_p2p_headers.py` announces ZIP 259's NU7 protocol version for each network: 170180 on Testnet, 170190 on
  Mainnet, where peers drop lower versions once NU7 activates.

## v0.1.0 (2026-10-29)

- The Zcash header relay, extracted from [Zecured](https://github.com/Himess/zecured) at `73de9f9` with its commit
  history. Its Solidity, tests, fixtures, tools, deploy script, parameters and feeder are unchanged.
- New: `IZcashHeaderRelayReader` (the read side, for any 0.8 compiler), the `ZcashFinalityGate` example, the ABI in
  `abi/`, docs/INTEGRATING.md and docs/RELAYING.md.
- New: a relay on Base Sepolia that follows Zcash Testnet from block 4,554,953
  (`contracts/deployments/seam-base-sepolia.relay.json`).
