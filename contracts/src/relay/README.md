# Zcash header relay

`ZcashHeaderRelay` accepts the block headers of one Zcash network, **Testnet or Mainnet**, on Base and keeps the best
chain by cumulative work, starting from a trusted checkpoint. The network is fixed at deployment by its consensus
parameters (`RelayParams`, from `contracts/config/relay-<network>.json`) and reported by `networkId()`. Zecured used it
to prove deposits on Base without trusting its signers
([Zecured's TIER2.md](https://github.com/Himess/zecured/blob/73de9f909e270e8aea10c2b221905f62a6a4c4df/docs/TIER2.md)).

| File | |
|---|---|
| `ZcashHeaderRelay.sol` | the relay: network parameters, contextual checks, storage, fork choice |
| `ZcashEquihash.sol` | Equihash (200, 9) verification on the EIP-152 BLAKE2b precompile (0x09) |
| `ZcashPow.sol` | nBits encoding, work, the difficulty adjustment formula, SHA256d block hash |
| `IZcashHeaderRelay.sol` | interface, events and errors for consumers; `ZCASH_TESTNET` and `ZCASH_MAINNET` |

## Networks

Only the rules that differ between the networks are parameters; everything else (Equihash (200, 9), version ≥ 4, the
11-block median, damping 4, bounds +16 % / −32 %, the 90-minute and 2-hour time limits, the storage layout) is the
same code for both. Every value was checked in Zebra v7.0.0-rc.0 and the ZIPs; the sources, with links, are on
`ZcashHeaderRelay.RelayParams` and in
[Zecured's docs/ref/FACTS.md](https://github.com/Himess/zecured/blob/73de9f909e270e8aea10c2b221905f62a6a4c4df/docs/ref/FACTS.md).

| `RelayParams` | Testnet | Mainnet |
|---|---|---|
| `networkId` | `keccak256("zcash-testnet")` | `keccak256("zcash-mainnet")` |
| `powLimit` / `powLimitBits` | 2^251 − 1 rounded: `0x07ffff << 232` / `0x2007ffff` | 2^243 − 1 rounded: `0x07ffff << 224` / `0x1f07ffff` |
| `minDifficultyBlocks` / `minDifficultyStartHeight` | yes, from 299,188 | no (refused with the Mainnet id) / 0 |
| `maxTimeStartHeight` (time ≤ median-time-past + 90 min) | 653,606 | 0: every height |
| `nu7ActivationHeight` | 4,465,026 | 3,543,405 (ZIP 259, Zebra 7.0.0); `NEVER` = 2^32 − 1 would switch NU7 off |
| target spacing / averaging window before NU7 | 75 s / 17 | 75 s / 17 |
| target spacing / averaging window from NU7 | 25 s / 102 | 25 s / 102 |
| `minCheckpointHeight` | 653,606 | 653,600 (Blossom) |

The constructor checks that the parameters can be applied consistently (`powLimitBits` is the canonical compact form
of `powLimit`, below 2^255; no minimum-difficulty rule with the Mainnet id; non-zero spacings; windows of 1 to 102
blocks, which the 113 context entries cover; checkpoints at height 112 or above) but does not pin the public networks'
values: those come from the configuration files, and `params()` returns them from a deployed relay. A Mainnet relay
deployed with `NEVER` expects the pre-NU7 nBits for the first NU7 block, refuses it and can extend nothing above it,
so a relay that must cross NU7 is deployed with the assigned height.

## What is verified

Every header must pass all of the following (references: Zebra v7.0.0-rc.0, commit
`6d1e414d6f55e4180d0e47baaa934bf97d5b4fec`, the Zebra version Zecured's light wallet server ran):

1. **Format and linkage.** 1487 bytes (140-byte prefix, compactSize(1344), solution); version ≥ 4 as an int32 (Zebra
   `check_version`); the parent (`hashPrevBlock`) is known; the block is not already known.
2. **Time rules** (Zebra `difficulty_threshold_and_time_are_valid`): time > median-time-past of the 11 previous blocks;
   time ≤ that median + 90 min (from `maxTimeStartHeight`: Testnet 653,606, Mainnet every height); time ≤
   `block.timestamp` + 2 h (Zebra's local-clock rule `time_is_valid_at`, applied with Base's clock).
3. **Difficulty** (Zebra `AdjustedDifficulty`, ZIP 218, ZIP 259): nBits must equal `ThresholdBits(height)`: mean target
   over the averaging window (exact floor, computed as Zebra does with separate quotient and remainder sums), damping
   factor 4, bounds +16 % / −32 %, medians of 11 times, divide-then-multiply, capped at the network's PoWLimit. The
   window is **17 blocks at 75 s before NU7 and 102 blocks at 25 s from NU7** (Testnet height 4,465,026; Mainnet
   3,543,405). **Testnet minimum-difficulty rule** (`minDifficultyBlocks`, never on Mainnet): if a block's time is
   *more than* 450 s after its parent's (6 × 75 s before NU7, 18 × 25 s from NU7), nBits must be `0x2007ffff`.
4. **Equihash (200, 9)**, following librustzcash `components/equihash/src/verify.rs`: BLAKE2b-400 with
   personalization `"ZcashPoW" || le32(200) || le32(9)` over the 140-byte prefix and `le32(i / 2)`; 512 big-endian
   21-bit indices; collisions on the leading 20 bits at each of the 9 levels, ordering (left subtree's first index <
   right's), distinct indices, zero root. The first 128-byte BLAKE2b block is compressed once; each index costs one
   final-block compression. The library's header comment explains why its packed representation is equivalent to the
   reference; the reported error kind matches the reference for single-defect solutions.
5. **Proof of work**: SHA256d(header), read little-endian, ≤ the target decoded from nBits.
6. **Fork choice**: a block becomes the tip only with strictly more cumulative work than the tip
   (work = 2^256 / (target + 1)); ties keep the first block seen. Reorganisations of any depth are accepted (Zebra
   itself refuses to reorg below `MAX_BLOCK_REORG_HEIGHT` = 1000 blocks).

The NU7 rules were checked against real data, not only taken from the ZIPs: the Python port of the rules
(`test/relay/tools/zcash_header_check.py`) reproduces the nBits and passes the time rules for **all 23,400 Testnet
headers from 4,464,800 to 4,488,199** (225 pre-NU7, 23,174 post-NU7), and every one of them passes Equihash and PoW. Changing the
window to 17, the spacing to 75 s, the activation height by one block, the damping, the rounding of the damping
(floor instead of truncation) or the order of division and multiplication each breaks real blocks. One detail is not
pinned down by the data: whether the mean adds the remainder term (it changes no real nBits after compact rounding);
the relay follows Zebra's source. No minimum-difficulty block occurred in that range (largest gap 434 s), so that
rule is covered by synthetic headers with genuine Equihash solutions.

The Mainnet rules were checked the same way on **1,600 consecutive Mainnet headers, 3,497,446 .. 3,499,045** (fetched
over P2P; their SHA256d linkage reaches the hashes Zebra v7.0.0-rc.0's `main-checkpoints.txt` lists at 3,497,445,
3,497,845, 3,498,245, 3,498,645 and 3,499,045). With the Mainnet parameters the Python model passes every time,
nBits, Equihash and PoW check of the 1,572 headers that have their 28-block context. Of the 1,487 that have a full
113-block context, a window of 16, 18 or 102 blocks or a 25 s spacing reproduces at most 2 nBits, and the Testnet
minimum-difficulty rule would refuse exactly the 7 blocks that came more than 450 s after their parents (451 s to
762 s), which carry the ordinary retarget value. The
Mainnet PoWLimit itself is not exercised: real targets are about 1.8 × 10^8 times below it. The post-NU7 rules are the
same on both networks (ZIP 218), so on Mainnet they are covered by the Testnet data only until Mainnet's NU7 activates
(about 6 November 2026). Once both networks are past NU7, only the PoWLimit and the minimum-difficulty rule tell their
headers apart, so the network a relay follows is the one its checkpoint is on.

## Not verified (yet)

- **Block contents.** Transactions, `hashMerkleRoot` and `hashBlockCommitments` are stored as committed by the proof
  of work but not checked against block data. Proving facts about them (an Ironwood root, a transaction) is the job of
  the inclusion proofs designed in
  [Zecured's TIER2.md](https://github.com/Himess/zecured/blob/73de9f909e270e8aea10c2b221905f62a6a4c4df/docs/TIER2.md).
- **The checkpoint.** Its hash, height and the 113 context entries (times and nBits of heights checkpoint − 112 ..
  checkpoint) are trusted constructor input.
- **The network parameters.** Like the checkpoint, they are trusted deployment input (`config/relay-<network>.json`);
  the constructor refuses inconsistent sets, not wrong values. NU7's Mainnet height, 3,543,405 (ZIP 259, Zebra
  7.0.0), is checked on real Mainnet headers only once NU7 activates there. The NatSpec of
  `RelayParams.nu7ActivationHeight` in `ZcashHeaderRelay.sol` predates that assignment: the configuration holds the
  height.
- **Finality.** No checkpoint finality or reorg depth limit beyond the most-work rule.

**Testnet caveat.** The minimum-difficulty rule lets anyone mine a block at difficulty 1 when its time is more than
450 s after its parent's. Such a block adds 32 units of work, a normal post-NU7 block about 380,000. The 2-hour
clock rule bounds how far an attacker can push timestamps, but blocks of that kind can still extend the best chain
cheaply, and once the averaging window holds such blocks the following ones stay easy too: in a simulation, 300
extra confirmations cost 0.085 of one honest block's work. `confirmations` counts them as blocks;
`confirmationWork` does not inflate. A consumer that needs reorg resistance on Testnet must require work, never a
block count ([Zecured's TIER2.md](https://github.com/Himess/zecured/blob/73de9f909e270e8aea10c2b221905f62a6a4c4df/docs/TIER2.md)).
Mainnet has no minimum-difficulty blocks; work is still the measure to require there.

**Reorganisations.** A reorganisation rewrites the best-chain index of the new branch only; heights above the new
tip are left as stale entries that every view ignores (all reads are bounded by the tip height). Clearing them used
to cost gas per orphaned height, so a long, light branch built on a stale tip could push the honest reorganisation
above the transaction gas limit and freeze the relay (Zecured's contracts review, R-1).

## Gas

Measured with `forge test --isolate` (each submission its own transaction, cold storage), solc 0.8.28, optimizer 200
runs, over the 32 real post-NU7 headers of `testnet_recent.json` and the first 32 headers of `mainnet_recent.json`:

| | gas |
|---|---|
| `submitHeader`, extending the best chain: whole transaction (21,000 + 23,796 calldata for 1,572 bytes + execution), Testnet after NU7 (102-block window) | min 613,810 · **mean 625,703** · max 664,744 |
| the same on Mainnet before NU7 (17-block window: 3 cold slots for the mean target instead of 13) | min 566,896 · **mean 579,490** · max 614,918 |
| execution of one typical header (trace of Testnet header 4,488,005) | 576,543 |
| ↳ Equihash | 361,100: 512 index decodes with duplicate check and 513 BLAKE2b compressions (274,120), tree check (82,843) |
| ↳ time rules and difficulty adjustment | ≈ 76,500: mean target over 102 blocks (56,766, mostly 13 cold storage reads), two medians (≈15,400) |
| ↳ storage (4-slot record, best-chain indexes, tip) and events | ≈ 134,000 |
| fork test, whole transactions: minimum-difficulty blocks (no mean target to compute), side branches, reorganisations of up to 8 blocks | 517,572 – 656,865 |
| deployment, including the 113 context entries and the 12 parameters | 3,253,717 |

The network parameters are immutables: against the relay with Testnet constants
([Zecured a577690](https://github.com/Himess/zecured/commit/a577690da8ba77de8decf5c4850700f8c20dbdca)), the same 32 Testnet headers
cost **101 gas more per header** (mean 624,849 → 624,950 in one `--isolate --gas-report` run with both relays) and
the deployment 316,469 more (the parameter checks and `params()`; runtime code 8,578 → 9,862 bytes).
`forge test --isolate --match-path 'test/relay/*' --match-test gas -vv` prints the per-call numbers; the deployment
cost needs `FOUNDRY_DYNAMIC_TEST_LINKING=false ... --gas-report` (with dynamic test linking `new` becomes a
cheatcode and the report shows 0). At 25-second blocks, relaying every header is ~3,456 transactions per day
(≈ 2.2 × 10^9 gas). Proving batches of headers in a zkVM instead is discussed in
[Zecured's TIER2.md](https://github.com/Himess/zecured/blob/73de9f909e270e8aea10c2b221905f62a6a4c4df/docs/TIER2.md).

## Tests

```bash
cd contracts
forge test --match-path 'test/relay/*'            # 80 tests, about 1 s once built; one needs BASE_MAINNET_RPC_URL
forge test --isolate --match-path 'test/relay/*'  # same, every top-level call its own transaction
```

Every test deploys through `RelayFixtures`, which holds the two networks' parameters (`_testnetParams()`,
`_mainnetParams()`, equal to the configuration files) and picks them from the fixture's `network`. Tests that expect a
constructor revert deploy through the external `this.deployRelay(...)`: Foundry 1.8 links tests dynamically by default,
`new` then becomes a cheatcode, and a constructor revert awaited by `vm.expectRevert` ends the test early as a pass,
silently skipping every later check (before this change `test_constructor_rejectsBadInput` checked only its first case).

- `ZcashHeaderRelay.t.sol` (27, Testnet parameters): the 32 recent post-NU7 headers and the 30 headers across the NU7
  activation, each submitted and checked (tip, height, confirmations, stored fields); batch submission; rejections:
  unknown parent, duplicate, length and solution size, version, time ≤ median-time-past, time > median + 90 min, time >
  clock + 2 h, wrong nBits, the minimum-difficulty rule both ways (a 451 s gap requires `0x2007ffff`; a block 30 s or
  exactly 450 s after its parent with a genuine solution at `0x2007ffff` is rejected), Equihash bit flips in solution,
  nonce and header, swapped leaves and subtrees, duplicated indices, a valid solution whose hash is above the target;
  fork choice: a lighter fork does not reorganise, an equal-work fork keeps the first tip, a heavier fork (8 blocks
  over 7) reorganises and the real block then reorganises back; a side-branch block is checked against its own
  branch's history; constructor input validation; gas.
- `ZcashHeaderRelayMainnet.t.sol` (17, Mainnet parameters): the 180 real Mainnet headers 3,497,560 .. 3,497,739, each
  submitted and checked, including the two that came 451 s and 466 s after their parents; batch submission; cumulative
  work; rejections: wrong nBits (also the Mainnet PoWLimit after a 451 s gap: no minimum-difficulty rule), time ≤
  median-time-past, time > median + 90 min, time > clock + 2 h, six one-defect Equihash mutations, a genuine solution
  whose hash is above the Mainnet target; where the networks differ: Testnet parameters refuse the Mainnet block that
  came 451 s after its parent, a genuine Testnet minimum-difficulty block is refused without the rule, Mainnet
  parameters refuse a real Testnet header from between the two NU7 heights and a context holding a Testnet
  minimum-difficulty block, and compute a real post-NU7 Testnet header's nBits exactly (both networks then share the
  adjustment), an NU7 height set too early breaks the real Mainnet block there; gas.
- `ZcashHeaderRelayParams.t.sol` (13): the network ids, `params()`, the PoWLimits as the rounded specification
  values; every constructor refusal (`InvalidNetworkId`, `InvalidPowLimit` for non-canonical, foreign, negative, zero
  and ≥ 2^255 limits, `InvalidMinDifficultyRule`, `InvalidDifficultyAdjustment`, `InvalidMinCheckpointHeight`) and the
  checkpoint height bounds; that `maxTimeStartHeight`, `minDifficultyStartHeight` and `nu7ActivationHeight` are applied
  at exactly their heights; the configuration files parse to the tested parameters, deploy working relays and refuse a
  checkpoint of the other network; `DeployRelay`'s record name and NU7 height checks.
- `ZcashHeaderRelayReview.t.sol` (3): the regressions of Zecured's contracts review (batch skipping known headers,
  reorganisation cost independent of the orphaned branch, stale entries above the tip).
- `RelayCostMainnet.t.sol` (2): gas per Mainnet header by batch size (1, 4, 8, 16); with `BASE_MAINNET_RPC_URL`, the
  L2 execution fee and the L1 data fee of each batch on a Base mainnet fork (skipped without it).
- `ZcashEquihash.t.sol` (9): the hard-coded BLAKE2b initial state recomputed from the IV and parameter block; the
  EIP-152 precompile against its test vector 5; leaf strings against Python's `hashlib.blake2b`; all 62 real solutions;
  the mutations with the reference's error kinds; crafted trees for every error kind (including `NonZeroRootHash`); a
  fuzzed bit flip anywhere in a real solution.
- `ZcashEquihashCorpus.t.sol` (1): agreement with the `equihash` crate 0.3.0 (the reference Zebra calls) on 592
  labelled cases built from the real Testnet headers.
- `ZcashPow.t.sol` (8): nBits decoding and encoding against zcashd `SetCompact`/`GetCompact` vectors, PoWLimit, work,
  damping truncation, clamps and cap, median, byte reversal, and a fuzzed compact round trip.

`foundry.toml` has read-only `fs_permissions` entries for `./test/relay/fixtures` and `./config`.

## Fixtures

| File | Content | Source |
|---|---|---|
| `testnet_recent.json` | checkpoint 4,487,999 + 113 context entries; 32 headers 4,488,000 .. 4,488,031 | Zcash Testnet P2P |
| `testnet_nu7_activation.json` | checkpoint 4,465,014 + context; 30 headers 4,465,015 .. 4,465,044 (11 pre-NU7, 19 post-NU7) | Zcash Testnet P2P |
| `testnet_synthetic.json` | **SYNTHETIC**: one-defect mutations of header 4,488,000; fork headers on block 4,488,002 | generated |
| `testnet_deposit_anchor.json` | checkpoint 4,488,559 + context; 12 headers 4,488,560 .. 4,488,571: the anchor of the real deposit proof in Zecured's [`test/fork/DepositProofs.t.sol`](https://github.com/Himess/zecured/blob/73de9f909e270e8aea10c2b221905f62a6a4c4df/contracts/test/fork/DepositProofs.t.sol) and the work on top of it | Zcash Testnet P2P |
| `mainnet_recent.json` | checkpoint 3,497,559 + 113 context entries; 180 headers 3,497,560 .. 3,497,739 (before NU7), including 3,497,585 and 3,497,733, 451 s and 466 s after their parents | Zcash Mainnet P2P, pinned by Zebra's checkpoint list |
| `mainnet_synthetic.json` | **SYNTHETIC**: one-defect mutations of header 3,497,560; F1, a header on block 3,497,562 with the real child's time and nBits and a genuine solution whose hash is above the target | generated |

Raw headers come from the Zcash P2P networks themselves: Zecured's light wallet server leaves
`CompactBlock.header` empty and no public explorer exposes `hashBlockCommitments`. `tools/zcash_p2p_headers.py`
recomputes every SHA256d and checks the linkage while fetching. Testnet: `getheaders` from a `/Zebra:7.0.0-rc.0/` peer
found through the Testnet DNS seeders, protocol 170180. Mainnet (`--network mainnet`: magic `24e92764`, port 8233,
Zebra's five Mainnet DNS seeders): on 2026-10-15 only `/Zakura:1.5.0/` and `/Zakura:1.6.0/` peers (protocol 170190)
answered `getheaders`; the Zebra peers tried (4.3.0 to 6.4.2) closed the connection on it and a zcashd peer
(`/MagicBean:6.20.0/`) timed out. `tools/build_fixture.py --cipherscan` compared every fixture entry with the
independent CipherScan API of its network (hash, previous hash, time, nBits; for full headers also version, merkle
root, nonce and solution): all matched (Testnet on 2026-10-11, Mainnet on 2026-10-15). For Mainnet,
`--zebra-checkpoints` also requires the fetched chain to link, by SHA256d and `hashPrevBlock`, to the hashes Zebra
v7.0.0-rc.0's `main-checkpoints.txt` lists below and above the selection (3,497,445 and 3,497,845), so every entry of
`mainnet_recent.json` is pinned by Zebra's own checkpoint list. `tools/zcash_header_check.py fixture <file>`
re-verifies a fixture offline with its network's rules.

The synthetic fork headers are not Zcash blocks, but their Equihash solutions are genuine: they were found at the
Testnet minimum difficulty by Tromp's solver (`equihash` 0.3.0, `solver` feature) through
`research/relay-synthetic-headers`, which keeps only solutions the reference verifier accepts (the solver sometimes
returns candidates with duplicate indices). `tools/make_synthetic.py` re-verifies each one and records the expected
nBits and median-time-past from the Python model. The Mainnet F1 header is built the same way, with a solution whose
hash is above the Mainnet target.

Regenerating (network access for the fetch, the checkpoint list and `--cipherscan`; no step is needed to run the
tests):

```bash
cd contracts/test/relay/tools
# Testnet
python3 -I zcash_p2p_headers.py --from-hash <display hash of block h-1> --from-height <h-1> --count N --out all.jsonl
python3 -I build_fixture.py --headers all.jsonl --peers all.jsonl.peers --checkpoint-height H --count 32 \
    --description "..." --out ../fixtures/testnet_recent.json --cipherscan
(cd ../../../../research/relay-synthetic-headers && cargo build --release)
python3 -I make_synthetic.py ../../../../research/relay-synthetic-headers/target/release/relay-synthetic-headers ../fixtures
python3 -I zcash_header_check.py fixture ../fixtures/testnet_recent.json
# Mainnet: 400 headers from Zebra's checkpoint 3,497,445 up to the next one, 3,497,845
curl -fsSO https://raw.githubusercontent.com/ZcashFoundation/zebra/v7.0.0-rc.0/zebra-chain/src/parameters/checkpoint/main-checkpoints.txt
python3 -I zcash_p2p_headers.py --network mainnet --from-height 3497445 \
    --from-hash 000000000005a01768f53909bab552b8646acd66d080d688a6ffdde3b819032d --count 400 --out mainnet.jsonl
python3 -I build_fixture.py --network mainnet --headers mainnet.jsonl --peers mainnet.jsonl.peers \
    --checkpoint-height 3497559 --count 180 --zebra-checkpoints main-checkpoints.txt --cipherscan \
    --description "..." --out ../fixtures/mainnet_recent.json
python3 -I make_synthetic.py ../../../../research/relay-synthetic-headers/target/release/relay-synthetic-headers ../fixtures mainnet
python3 -I zcash_header_check.py fixture ../fixtures/mainnet_recent.json
python3 -I zcash_header_check.py chain mainnet.jsonl --network mainnet --equihash   # every header, Mainnet rules
```

## Deployment

`script/DeployRelay.s.sol` reads the network's parameters from `config/relay-<network>.json` (`RELAY_NETWORK`,
default `testnet`, or any file through `RELAY_CONFIG`) and the checkpoint from `RELAY_CHECKPOINT`, a file in the
fixture format whose `network` must match the configuration's. It writes `deployments/<name>.relay.json`, in a
simulation too (with a simulated address). **Always set `DEPLOYMENT_NAME`:** a Testnet relay's record otherwise defaults
to `deployments/base-sepolia.relay.json`, the record of Zecured's live run below, and any other network needs a name.
The script refuses an NU7 height that is not a multiple of 3 (ZIP 259) and warns when it is `NEVER`.

Two Testnet relays are recorded on Base Sepolia:

- **Zecured's live-run relay**
  [`0xEc34aA21cDe1366b397a99A4d90fc6B2f02cFaEa`](https://sepolia.basescan.org/address/0xEc34aA21cDe1366b397a99A4d90fc6B2f02cFaEa#code):
  checkpoint 4,547,001, deployed for Zecured's live run of 27 October 2026 (`deployments/base-sepolia.relay.json` and
  `base-sepolia.relay-checkpoint.json`). It stays unoperated during ZECATHON judging.
- **Seam's relay**
  [`0x77ceDADCFf8bE399C67AB579Ad739820FaC5cE0c`](https://sepolia.basescan.org/address/0x77ceDADCFf8bE399C67AB579Ad739820FaC5cE0c#code):
  checkpoint 4,554,953, deployed on 29 October 2026 (`deployments/seam-base-sepolia.relay.json` and
  `seam-base-sepolia.relay-checkpoint.json`, checked by `test/deployments/SeamBaseSepolia.t.sol`). It follows Zcash
  Testnet; the repository's [README](../../../README.md) says how it is fed.

Nothing here is a mock: the relay verifies real proof of work, and the synthetic data exists only in tests.
