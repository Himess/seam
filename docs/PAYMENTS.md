# Shielded payment proofs

A Seam payment proof lets a contract on an EVM chain check that a shielded Ironwood payment was made: to a committed
receiver, of at least an amount, for one reference, and in the Zcash chain a Seam relay follows. The payment's
transaction id, note and address never reach the EVM chain.

## 1. What a proof shows

A proof is a RISC Zero receipt (wrapped in Groth16) that the guest accepted a private witness. The guest runs
`seam_payment::verify`, which accepts the witness only if:

1. **Epoch.** Its consensus branch id is one the build allows, and the transaction carries the same one. The Testnet
   build allows NU7's; the Mainnet build allows NU6.3's and NU7's. The network is a build parameter, never a witness
   field, so each network has its own guest and image id.
2. **Shape.** The transaction is v6 and Ironwood-only, with outputs enabled: no transparent input or output, no
   Sapling spend or output, and no Orchard-pool action.
3. **Inclusion.** The transaction's txid reaches the `hashMerkleRoot` of its block B along a SHA-256d path. B's ZIP 221
   history-tree leaf, through its path and the peaks, with the auth data root of a later header T, reaches T's
   `hashBlockCommitments` (ZIP 244). Every node of that tree is personalized with the epoch's branch id, so T is in
   B's epoch, or is the next epoch's activation block, whose history ZIP 221 and Zebra still take from the previous
   epoch (read in their code; no real activation block has been tried as an anchor).
4. **The note.** The witness's opening (a 43-byte raw receiver, the value and rseed) rebuilds a V3 note (ZIP 2005)
   whose rho is the nullifier its action spends. The action's ciphertext, recovered with the ephemeral secret that note
   derives and its `pk_d`, yields exactly that note. So the receiver's wallet finds it and can spend it.
5. **The memo.** Its first 70 bytes are the ASCII text `seam1:` followed by 64 lowercase hex digits, which decode to
   `paymentRef`. The rest of the memo is free.
6. **The receiver commitment** is `keccak256(salt32 || receiver43)`.

The guest then commits the journal, 192 bytes:

```solidity
abi.encode(uint8 version = 1, uint32 branchId, bytes32 anchor, bytes32 paymentRef,
           bytes32 receiverCommitment, uint64 zatoshi)
```

`anchor` is SHA-256d of header T in internal byte order, as the relay stores it, and `zatoshi` the note's value.

## 2. What reaches the EVM chain, and what never does

| On chain | Never on chain |
|---|---|
| the anchor T, a header the relay already holds | the transaction id |
| `paymentRef`, chosen by the app | the payment's block B, hidden among the blocks between the order and T |
| the receiver commitment `keccak256(salt32 \|\| receiver43)` | the note commitment and the nullifier |
| the value, in zatoshi, and the branch id | the receiver, its address and viewing key, and the salt |

The value is public: an app whose amounts must stay private needs another design. The salt must be fresh, random and
secret for each payment: a reused salt and receiver link payments on chain, and a guessable salt lets anyone test
candidate receivers against the commitment. `seam commitment --address <ua> --salt-file <file> --new-salt` makes such a
salt, in a file only its owner may read, and prints the commitment an app commits to.

## 3. What you have to trust

- **RISC Zero's proof system,** and the governance of the RISC Zero verifier router the verifier calls. The router's
  owner can add a verifier under a new selector or remove one, but never replace the verifier behind a selector
  (`RiscZeroVerifierRouter.addVerifier` refuses a selector in use or removed). The payment verifier pins the Groth16
  selector it accepts, so a seal is checked by that one verifier or not at all. A removal, or the emergency stop RISC
  Zero puts in front of each verifier (its guardian can trigger it at once), would stop proofs from verifying: a risk
  to liveness, not to safety. An app that pins the verifier then needs a new one, and its open orders run towards
  their refund meanwhile, even those already paid.
- **The pinned image id.** It is RISC Zero's image id of the guest's ELF, built reproducibly in RISC Zero's Docker
  builder; anyone can rebuild it and compare (`zkvm/pinned-image-ids.txt` also lists the ELF's SHA-256).
- **What the guest is built from:** pre-release Zcash crates (`zcash_primitives` 0.31.0-pre.1, `zcash_protocol`
  0.11.0-pre.0), and a patched `pasta_curves` that only the zkVM build compiles (`zkvm/vendor/pasta_curves`,
  `SEAM_PATCH.md`): the native tests never run the patch; `seam parity` does.
- **The Groth16 trusted setup** of RISC Zero's verifier.
- **The relay.** The anchor must be on its best chain with enough work on top
  ([INTEGRATING.md](INTEGRATING.md) § 4). The relay checks every header, but someone has to feed it.
- **The statement.** It checks the rules above and nothing else: no proof of work (the relay does) and no Halo 2 proof
  or signature (Zcash's consensus did, when the block was mined into the chain the relay follows). A chain an attacker
  mines in private can carry any transaction, so the work an app requires on top of an anchor must outweigh what all
  its open payments are worth together: one deep enough reorganisation could fake a payment for each of them.
- **What has been proven so far.** One real payment, on Testnet, made with `seam pay`. The Mainnet guest is pinned but
  has proven no real Mainnet payment: prove one, against real NU6.3 and NU7 history, before a verifier pins it on
  Mainnet. No payment from another wallet has been proven yet.

## 4. How to pay

Any wallet that can send an Ironwood payment with a memo:
- to the receiver the app committed to;
- of at least the amount;
- with the memo text `seam1:` followed by the 64 lowercase hex digits of `paymentRef`.

ZIP 302 reads a memo whose first byte is 0xF4 or less as UTF-8 text, so wallets show `seam1:…` as text and can send
it typed by hand. A ZIP 321 payment URI carries it in its `memo` parameter: the memo's bytes in base64url without
padding. `seam memo <paymentRef> --address <ua> --amount <ZEC>` prints both.

The payment must be Ironwood-only: a wallet that adds a transparent change output, or spends from or pays into another
pool (the Orchard pool uses the same address format), makes a transaction the statement refuses. And the whole amount
must arrive in one note: two notes of half the amount are two payments of half.

**Check the receiver before paying.** The commitment names one exact receiver: a payment to any other address of the
same wallet (another diversifier, its default address) arrives, but never proves the order. So get the salt and the
address, and check that `seam commitment --address <ua> --salt-file <file>` prints the order's `receiverCommitment`.
A receiver who won't share the salt can't be paid safely.

## 5. Who can prove

The witness needs the note's opening and the transaction's place in the chain. Two parties have the opening:
- **the payer,** who built the transaction and kept the opening (with `ovk = None`, nobody else can recover the note
  from the chain);
- **the receiver,** who finds the note with its viewing key by trial decryption.

Today a payer can prove only after paying with `seam pay`, which keeps the opening; it pays on Testnet only. Most
wallets don't export a note's opening, and `seam witness` can't yet recover a note with the payer's outgoing viewing
key. So on Mainnet, for now, only the receiver can prove: an app whose payer must prove has to wait for that.

Either one builds the witness locally with `seam witness`, from chain data any node or lightwalletd server serves.
Proving runs on any machine with the witness; a witness should never leave a machine its owner doesn't control.
`seam` refuses an environment that would hand the witness to Bonsai, RISC Zero's remote prover (`BONSAI_API_URL`, or
`RISC0_PROVER=bonsai`), and is built so that `RISC0_DEV_MODE` can't fake a receipt.

`seam witness` asks the light wallet server for the payment's transaction, by its id, and for its block: that server
learns which payment is being proven, and from where. Use a server you run, or reach one over Tor, when that matters.

## 6. How an app consumes a proof

`ZcashPaymentVerifier.verify(journal, seal)` returns the payment and the work on top of its anchor. It reverts unless
the journal is a version 1 journal of a branch id the verifier allows, the router verifies the seal for the pinned
image id, and the anchor is on the relay's best chain. It is stateless: it accepts the same proof any number of times,
and one payment under many anchors, so an app binds each `paymentRef` to one obligation and settles it once. Finality
is the app's choice:
- **Work, not blocks:** require `anchorWork >= minWork`, fixed at deployment ([INTEGRATING.md](INTEGRATING.md) § 4).
- **A pending window:** settle only after `finalizeDepth` more headers and `finalizeDelay` seconds, and only if the
  anchor is still final then.
- **Reorganisations:** drop a pending settlement whose anchor left the best chain; anyone can prove again.
- **A deadline, judged on the Zcash side:** a payment counts if its anchor's header time is no later than the
  deadline (the payment's block comes before the anchor, so a payer should pay a few blocks ahead of it). Give provers
  a grace period after the deadline, and after a dropped settlement, so a slow proof or a lagging relay never costs a
  payer who paid in time; refund the EVM side only once that window has closed with nothing pending. Size that grace
  period against the time `minWork` takes to pile up ([INTEGRATING.md](INTEGRATING.md) § 10).

`ShieldedPaymentEscrow` is an example of all four.

## 7. Choosing the anchor

The anchor reveals a header, so it tells an observer the payment is in some block below T, after the order that
published its reference. To keep that set large:
- **Fix the anchor height first,** then pay after a random delay, so the payment's time says nothing about the anchor.
- **Keep the payment and its anchor in one epoch.** The ZIP 221 history tree restarts at every network upgrade, so a
  payment made before an activation can only be proven against an anchor before it.
- **Don't pay near a known activation.**

`seam witness --anchor <height>` anchors at a planned header.

## 8. Cost

Gas measured on 31 October 2026 with `forge test --match-test test_proveThenFinalizePaysThePayee --gas-report` (solc
0.8.28, optimizer 200 runs), with the MOCK router and the MOCK token: each figure is the call's own execution, with
storage cold.

| Call | Gas |
|---|---|
| `ZcashPaymentVerifier.verify`, without the Groth16 check | 20,710: 19,882 its own (the journal, the selector, the relay's best chain and work), 828 the MOCK router |
| `ShieldedPaymentEscrow.open` | 226,061, with the token's transfer |
| `ShieldedPaymentEscrow.prove` | 118,243, with the `verify` above |
| `ShieldedPaymentEscrow.finalize` | 97,216, with the token's transfer |

The router's Groth16 check adds its own gas to `verify`. Measured on the real payment of 0.01 TAZ made on Zcash Testnet
on 30 October 2026, proven on 31 October 2026 on a rented RTX 4090 (24 GB; 30 vCPU, 98 GB of RAM) with the pinned
Testnet guest:

| Step | Measured |
|---|---|
| The guest, in the executor | 19,918,817 cycles in 21 segments of 2^20, 0.9 s |
| The succinct (STARK) receipt, on the GPU | 43.1 s |
| Its Groth16 wrap, in Docker on the same machine | 34.2 s; the seal is 260 bytes (the router's selector and the proof) |
| `verify` with the Groth16 check, through RISC Zero's router on a Base Sepolia fork | 256,720 gas (`gasleft()` around the call in `test_fork_theRealPaymentSettlesItsOrder`) |

The proof took 81 s of a machine rented at $0.52 an hour, about one cent. The whole session, with the setup and two
clean builds of both guests, took about 22 minutes, about $0.20. The seal, journal and image id are in
`contracts/test/examples/fixtures/testnet_real_payment.json`, which the fork test settles an escrow order with. The
witness proven there is the TEST-ONLY fixture of two disposable wallets; a real witness stays on a machine its owner
controls (§ 5).
