# Changelog

## v0.3.0 (unreleased)

Shielded payment proofs (docs/PAYMENTS.md), adapted from Zecured's payment path at `73de9f9`:
- `crates/seam-payment`: the statement a payment proof proves. The memo binds a text reference, `seam1:` and 64
  lowercase hex digits, that any wallet can send, and the journal opens with a version. Its K4 catalog: 52 single
  changes to a witness for the Testnet statement and 53 for Mainnet's, each with the outcome it must have.
- `zkvm/`: the RISC Zero guest (risc0 3.0.6), one per network, built in RISC Zero's Docker builder for reproducible
  image ids, and the vendored `pasta_curves` with Zecured's zkVM patch.
- `crates/seam-chain` and `crates/seam-cli`: the chain data a witness needs, from Zcash peers and a light wallet server;
  `seam memo`, `seam commitment`, `seam pay` (Testnet, a disposable wallet), `seam witness` (from the payer's opening or
  the receiver's viewing key) and, with the `zkvm` feature, the executor, the prover and the Groth16 wrap.
- `ZcashPaymentVerifier`: checks a proof's seal through the RISC Zero router, for a pinned guest and selector, and its
  anchor on a Seam relay's best chain. No owner, no upgrade.
- `ShieldedPaymentEscrow` (example, not audited): an ERC-20 locked until a shielded ZEC payment for the order is
  proven, with the payment's time judged on the Zcash side.
- `zkvm/pinned-image-ids.txt` pins both guests: Testnet `0x783f02ce…d2b3` and Mainnet `0xf8a08bff…83ca`. Each was
  built twice from scratch in RISC Zero's Docker builder, to the same ELF.
- A real payment of 0.01 TAZ, proven on an RTX 4090 (19.9 M cycles; the STARK in 43 s, its Groth16 wrap in 34 s),
  settles the example escrow's order on a Base Sepolia fork, through RISC Zero's router and Seam's Testnet relay
  (`test/examples/ShieldedPaymentEscrowFork.t.sol`). `verify` costs 256,720 gas with the Groth16 check.
- After an independent review on 31 October 2026:
  - the example escrow adds `refundTo` for an opener who can't receive the token, refuses itself as a payee or a
    destination, bounds its windows (`MAX_WINDOW`) and deadlines so nothing can overflow, and takes the work on top
    of the anchor from `verify`; a fork test shows the real seal refuses a changed journal;
  - `seam witness` checks that the server sent the transaction asked for; the CLI never overwrites a file it writes,
    and keeps file contents out of its errors;
  - `abi/IZcashPaymentVerifier.json`;
  - PAYMENTS.md and INTEGRATING.md say what a payer must check before paying, who can prove today, what the light
    wallet server learns, what has been proven so far, and why Testnet secures nothing.

## v0.2.0 (unreleased)

- `scripts/relay-feed.sh` (after the same review): the network must read as one of the two Zcash ids, or the feeder
  stops (a failed read used to feed a Mainnet relay from Testnet peers); a block locator follows a reorganisation at
  any depth (it stepped back at most 20 blocks); every wait on a peer is bounded; a refused header no longer reverts
  the round; one line per round, and a stop after an hour without progress; the RPC URL and, with a keystore, the key
  stay off command lines and out of the log. `zcash_p2p_headers.py` takes `--locator`.

- `config/relay-mainnet.json` sets NU7's Mainnet height to 3,543,405 (ZIP 259, Zebra 7.0.0) instead of `NEVER`, so a
  Mainnet relay deployed from it follows Mainnet across NU7. The test parameters, the Python reference and
  `build_fixture.py` use the same height.
- `zcash_p2p_headers.py` announces ZIP 259's NU7 protocol version for each network: 170180 on Testnet, 170190 on
  Mainnet, where peers drop lower versions once NU7 activates.
- `scripts/relay-feed.sh` reads the relay's `networkId()` and feeds a Mainnet relay from Mainnet peers.
- New: a relay on Base Sepolia that follows Zcash Mainnet from block 3,534,866
  (`contracts/deployments/seam-base-sepolia-mainnet.relay.json`), checked like the Testnet one by
  `test/deployments/SeamBaseSepoliaMainnet.t.sol`.

## v0.1.0 (2026-10-29)

- The Zcash header relay, extracted from [Zecured](https://github.com/Himess/zecured) at `73de9f9` with its commit
  history. Its Solidity, tests, fixtures, tools, deploy script, parameters and feeder are unchanged.
- New: `IZcashHeaderRelayReader` (the read side, for any 0.8 compiler), the `ZcashFinalityGate` example, the ABI in
  `abi/`, docs/INTEGRATING.md and docs/RELAYING.md.
- New: a relay on Base Sepolia that follows Zcash Testnet from block 4,554,953
  (`contracts/deployments/seam-base-sepolia.relay.json`).
