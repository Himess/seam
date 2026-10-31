# Extraction map

Where each part of Seam comes from in [Zecured](https://github.com/Himess/zecured) (commit `73de9f9`), and when the
rest moves here. The last row lists what Seam never takes.

| Zecured | Seam | When |
|---|---|---|
| `contracts/src/relay/` | same path | v0.1.0, unchanged (README text updated) |
| `contracts/test/relay/` (tests, fixtures, header tools) | same path | v0.1.0, unchanged; v0.2.0: NU7's Mainnet height, the P2P version per network, one test rebuilt and one added |
| `contracts/script/DeployRelay.s.sol`, `contracts/config/relay-*.json` | same paths | v0.1.0, unchanged; v0.2.0: NU7's Mainnet height in `relay-mainnet.json`, and the script needs `DEPLOYMENT_NAME` and never overwrites a record |
| `contracts/deployments/base-sepolia.relay*.json` | same paths | v0.1.0: the relay of Zecured's live run, 27 October 2026 |
| `scripts/relay-feed.sh`, `research/relay-synthetic-headers/` | same paths | v0.1.0; v0.2.0: the feeder follows the relay's network, with a block locator and bounded waits (CHANGELOG) |
| (new) | `contracts/src/interfaces/`, `contracts/src/examples/`, `contracts/test/examples/`, `contracts/test/deployments/`, `abi/`, `docs/INTEGRATING.md`, `docs/RELAYING.md`, `contracts/deployments/seam-base-sepolia.relay*.json` | v0.1.0 |
| (new) | `contracts/deployments/seam-base-sepolia-mainnet.relay*.json`, `contracts/deployments/seam-base-sepolia.payment-verifier.json`, `contracts/script/DeployPaymentVerifier.s.sol`, `CLAUDE.md` | v0.2.0 |
| (new) | `docs/PAYMENTS.md`, `contracts/test/payment/`, `contracts/test/examples/ShieldedPaymentEscrow.t.sol`, `seam memo` | v0.2.0 |
| `tier2/payment-statement`, `tier2/payment-k4`, the inclusion checks of `tier2/statement` | `crates/seam-payment` | v0.2.0, adapted: a text memo binding, a versioned journal, K4 in `k4.rs` |
| `tier2/methods/payment`, `tier2/vendor/pasta_curves` | `zkvm/guest`, `zkvm/methods`, `zkvm/vendor/pasta_curves` | v0.2.0 |
| `tier2/host` (payment parts) | `crates/seam-cli`, the `zkvm` feature | v0.2.0 |
| `signer/crates/deposit-witness` (`history.rs`, `mmr.rs`, `p2p.rs`, `chain.rs`, `witness.rs`, `payment.rs`) | `crates/seam-chain` | v0.2.0 |
| `signer/crates/zcash-ops` (`lwd.rs`, `scan.rs`, `spend.rs`), `swap/core/src/pay.rs` | `crates/seam-chain` (`lwd`, `scan`, `pay`) | v0.2.0 |
| `signer/crates/deposit-witness/src/bin/build_payment_witness.rs` | `crates/seam-cli` (`seam witness`) | v0.2.0 |
| `contracts/src/ZecuredSwap.sol` (payment path), `contracts/src/ZecuredVersions.sol` (pinning) | `contracts/src/payment/`, `contracts/src/examples/ShieldedPaymentEscrow.sol` | v0.2.0, rewritten |
| `bots/src/relayer.rs` | `crates/seam-relayer` | roadmap 2 |
| `tools/leak-scan` | `tools/leak-scan` | later |
| `zkvm/vendor/pasta_curves` | an upstream PR, or a separate patch crate | roadmap 4 |
| Stays in Zecured | FROST signers and vaults, Registry, Intake, zZEC, bonds, slasher, compensator, price feed, paymaster, app and site, the deposit and violation statements | — |
