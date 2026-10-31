# Changelog

## v0.2.0 (2026-10-31)

**Zcash Mainnet, set for NU7:**
- `config/relay-mainnet.json` sets NU7's Mainnet height to 3,543,405 (ZIP 259, Zebra 7.0.0) instead of `NEVER`, so a
  Mainnet relay deployed from it follows Mainnet across NU7. The test parameters, the Python reference and
  `build_fixture.py` use the same height.
- New: a relay on Base Sepolia that follows Zcash Mainnet from block 3,534,866
  (`contracts/deployments/seam-base-sepolia-mainnet.relay.json`), checked like the Testnet one by
  `test/deployments/SeamBaseSepoliaMainnet.t.sol`. It crosses NU7 around 6 November 2026 with no redeploy.
- `zcash_p2p_headers.py` announces ZIP 259's NU7 protocol version for each network: 170180 on Testnet, 170190 on
  Mainnet, where peers drop lower versions once NU7 activates.

**The feeder** (`scripts/relay-feed.sh`):
- It reads the relay's `networkId()` and feeds a Mainnet relay from Mainnet peers. The id must be one of the two Zcash
  networks', or the feeder stops: a failed read used to feed a Mainnet relay from Testnet peers.
- A block locator follows a reorganisation at any depth; it used to step back at most 20 blocks.
- Every wait on a peer is bounded, and every read of the relay is retried twice over a transient RPC error before
  the feeder stops.
- A refused header no longer reverts the round.
- It prints one line per round, and stops after an hour without progress.
- The RPC URL stays off command lines and out of the log, and so does the key with a keystore.
- It reads `.env` as settings instead of running it, and fills only what the environment leaves unset.
- It needs bash 4.4 and `timeout` (on macOS: `brew install bash coreutils`).
- `zcash_p2p_headers.py` takes `--locator`.

**Deploy scripts:** `DeployRelay.s.sol` needs `DEPLOYMENT_NAME`. It and `DeployPaymentVerifier.s.sol` refuse a name
whose record exists, and write the record only when they broadcast.

**Shielded payment proofs** (docs/PAYMENTS.md), adapted from Zecured's payment path at `73de9f9`:
- `crates/seam-payment`: the statement a payment proof proves.
  - The memo binds a text reference, `seam1:` and 64 lowercase hex digits, that any wallet can send.
  - The journal opens with a version.
  - Its K4 catalog: 59 single changes to a witness for the Testnet statement and 60 for Mainnet's, each with the
    outcome it must have.
- `zkvm/`: the RISC Zero guest (risc0 3.0.6), one per network, built in RISC Zero's Docker builder for reproducible
  image ids, and the vendored `pasta_curves` with Zecured's zkVM patch. `zkvm/pinned-image-ids.txt` pins both guests:
  Testnet `0x783f02ce…d2b3` and Mainnet `0xf8a08bff…83ca`. Each was built twice from scratch to the same ELF.
- `crates/seam-chain` and `crates/seam-cli`: the chain data a witness needs, from Zcash peers and a light wallet
  server. The commands:
  - `seam memo`;
  - `seam commitment`;
  - `seam pay` (Testnet, a disposable wallet);
  - `seam witness` (from the payer's opening or the receiver's viewing key);
  - with the `zkvm` feature: the executor, the prover and the Groth16 wrap.
- `ZcashPaymentVerifier`: checks a proof's seal through the RISC Zero router, for a pinned guest and selector, and its
  anchor on a Seam relay's best chain. No owner, no upgrade. `abi/IZcashPaymentVerifier.json`.
- `ShieldedPaymentEscrow` (example, not audited, not deployed): an ERC-20 locked until a shielded ZEC payment for the
  order is proven, with the payment's time judged on the Zcash side.
- A real payment of 0.01 TAZ (Zcash Testnet, 30 October 2026), proven on an RTX 4090:
  - 19.9 M cycles; the STARK in 43.1 s, its Groth16 wrap in 34.2 s;
  - it settles the example escrow's order on a Base Sepolia fork, through RISC Zero's router and Seam's Testnet relay;
  - `verify` costs 256,720 gas with the Groth16 check.
- **Live:** a `ZcashPaymentVerifier` on Base Sepolia, `0x1701e16350A3F0E29DC45E75F356eAc9dAB8d5f9`
  (`contracts/deployments/seam-base-sepolia.payment-verifier.json`, `script/DeployPaymentVerifier.s.sol`). It is bound
  to the Testnet relay and the pinned Testnet guest, and verifies the real payment's seal; the README shows the
  `cast call`.
- After an independent review on 31 October 2026:
  - the example escrow adds `refundTo`, refuses itself as a payee or a destination, and bounds its windows and
    deadlines;
  - `seam witness` checks that the server sent the transaction it asked for;
  - the CLI never overwrites a file it writes a secret to and checks those files before it starts, keeps file
    contents out of its errors, and never hands a witness to Bonsai;
  - seven more K4 changes;
  - the docs say what a payer must check before paying, who can prove today, what the light wallet server learns, and
    why Testnet secures nothing.

## v0.1.0 (2026-10-29)

- The Zcash header relay, extracted from [Zecured](https://github.com/Himess/zecured) at `73de9f9` with its commit
  history. Its Solidity, tests, fixtures, tools, deploy script, parameters and feeder are unchanged.
- New: `IZcashHeaderRelayReader` (the read side, for any 0.8 compiler), the `ZcashFinalityGate` example, the ABI in
  `abi/`, docs/INTEGRATING.md and docs/RELAYING.md.
- New: a relay on Base Sepolia that follows Zcash Testnet from block 4,554,953
  (`contracts/deployments/seam-base-sepolia.relay.json`).
