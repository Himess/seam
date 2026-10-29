# Extraction map

Where each part of Seam comes from in [Zecured](https://github.com/Himess/zecured) (commit `73de9f9`), and when the
rest moves here. The last row lists what Seam never takes.

| Zecured | Seam | When |
|---|---|---|
| `contracts/src/relay/` | same path | v0.1.0, unchanged (README text updated) |
| `contracts/test/relay/` (tests, fixtures, header tools) | same path | v0.1.0, unchanged |
| `contracts/script/DeployRelay.s.sol`, `contracts/config/relay-*.json` | same paths | v0.1.0, unchanged |
| `contracts/deployments/base-sepolia.relay*.json` | same paths | v0.1.0: the relay of Zecured's live run, 27 October 2026 |
| `scripts/relay-feed.sh`, `research/relay-synthetic-headers/` | same paths | v0.1.0 |
| (new) | `contracts/src/interfaces/`, `contracts/src/examples/`, `contracts/test/examples/`, `contracts/test/deployments/`, `abi/`, `docs/INTEGRATING.md`, `docs/RELAYING.md`, `contracts/deployments/seam-base-sepolia.relay*.json` | v0.1.0 |
| `bots/src/relayer.rs`, `signer/crates/deposit-witness/src/p2p.rs` | `crates/seam-relayer` | roadmap 2 |
| `signer/crates/deposit-witness` (the ZIP 221 history tree, `history_root`) | `crates/seam-history` | roadmap 3 |
| `tier2/payment-statement`, `tier2/payment-k4` | `crates/seam-payment` | roadmap 3 |
| `tier2/methods/payment`, `tier2/host` (payment parts) | `zkvm/` | roadmap 3 |
| `signer/crates/deposit-witness/src/bin/build_payment_witness.rs` | `crates/seam-cli` | roadmap 3 |
| `contracts/src/ZecuredSwap.sol` (payment path), `contracts/src/ZecuredVersions.sol` (pinning) | `contracts/src/payment/`, `contracts/src/examples/` | roadmap 3, rewritten |
| `tools/leak-scan` | `tools/leak-scan` | roadmap 3 |
| `tier2/vendor/pasta_curves` | an upstream PR, or a separate patch crate | roadmap 4 |
| Stays in Zecured | FROST signers and vaults, Registry, Intake, zZEC, bonds, slasher, compensator, price feed, paymaster, app and site, the deposit and violation statements | — |
