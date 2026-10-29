# Security

Seam is pre-release: testnets only, not audited. Don't secure real value with it.

## Report a vulnerability

Privately, through GitHub's private vulnerability reporting:
https://github.com/Himess/seam/security/advisories/new. Please don't open a public issue. There's no bug bounty.

What matters most:
- the relay accepting a header that Zcash's rules reject, or refusing one they accept;
- anything that lets a feeder change what a relay reports, beyond delaying it;
- anything in the docs or the example that would lead an integrator into an unsafe pattern.

## Scope

`contracts/src/relay/`, `contracts/src/interfaces/`, `contracts/src/examples/` (examples, not production code),
`contracts/script/DeployRelay.s.sol`, `scripts/relay-feed.sh` and `contracts/test/relay/tools/`.
