# Security

Seam is pre-release: testnets only, not audited. Don't secure real value with it.

## Report a vulnerability

Privately, through GitHub's private vulnerability reporting:
https://github.com/Himess/seam/security/advisories/new. Please don't open a public issue. There's no bug bounty.

What matters most:
- the relay accepting a header that Zcash's rules reject, or refusing one they accept;
- anything that lets a feeder change what a relay reports, beyond delaying it;
- a payment proof the verifier accepts for a payment that wasn't made as its journal says;
- anything that puts a payment's txid, note, receiver, salt or viewing key where it doesn't belong (a log, an error, an
  EVM chain, a third party);
- anything in the docs or the examples that would lead an integrator into an unsafe pattern.

## Scope

- **Contracts:** `contracts/src/relay/`, `contracts/src/interfaces/`, `contracts/src/payment/`, `contracts/src/examples/`
  (examples, not production code) and `contracts/script/DeployRelay.s.sol`.
- **Payment proofs:** `crates/seam-payment` (the statement), `zkvm/` (the guest, its build and the patched
  `pasta_curves`), `crates/seam-chain` and `crates/seam-cli`.
- **Tools:** `scripts/relay-feed.sh`, `scripts/gpu-prove.sh`, `contracts/test/relay/tools/` and the workflows in
  `.github/workflows/`.
