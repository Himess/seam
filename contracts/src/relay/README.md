# Zcash header relay (Tier 2 foundation)

`ZcashHeaderRelay` accepts Zcash **Testnet** block headers on Base and keeps the best chain by cumulative work,
starting from a trusted checkpoint. It is the first step towards proving Zecured deposits on Base without trusting
the signers ([docs/TIER2.md](../../../docs/TIER2.md)). It is a separate track: nothing in the Registry, the Adapter or
zZEC uses it yet.

| File | |
|---|---|
| `ZcashHeaderRelay.sol` | the relay: contextual checks, storage, fork choice |
| `ZcashEquihash.sol` | Equihash (200, 9) verification on the EIP-152 BLAKE2b precompile (0x09) |
| `ZcashPow.sol` | nBits encoding, work, the difficulty adjustment formula, SHA256d block hash |
| `IZcashHeaderRelay.sol` | interface, events and errors for consumers |

## What is verified

Every header must pass all of the following (references: Zebra v7.0.0-rc.0, commit
`6d1e414d6f55e4180d0e47baaa934bf97d5b4fec`, the Zebra version the project's light wallet server runs):

1. **Format and linkage.** 1487 bytes (140-byte prefix, compactSize(1344), solution); version ≥ 4 as an int32 (Zebra
   `check_version`); the parent (`hashPrevBlock`) is known; the block is not already known.
2. **Time rules** (Zebra `difficulty_threshold_and_time_are_valid`): time > median-time-past of the 11 previous blocks;
   time ≤ that median + 90 min (Testnet, from height 653,606); time ≤ `block.timestamp` + 2 h (Zebra's local-clock rule
   `time_is_valid_at`, applied with Base's clock).
3. **Difficulty** (Zebra `AdjustedDifficulty`, ZIP 218, ZIP 259): nBits must equal `ThresholdBits(height)`: mean target
   over the averaging window (exact floor, computed as Zebra does with separate quotient and remainder sums), damping
   factor 4, bounds +16 % / −32 %, medians of 11 times, divide-then-multiply, capped at PoWLimit (2^251 − 1 rounded to
   `0x2007ffff`). The window is **17 blocks at 75 s before NU7 and 102 blocks at 25 s from NU7** (Testnet height
   4,465,026). **Testnet minimum-difficulty rule**: if a block's time is *more than* 450 s after its parent's
   (6 × 75 s before NU7, 18 × 25 s from NU7), nBits must be `0x2007ffff`.
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

## Not verified (yet)

- **Block contents.** Transactions, `hashMerkleRoot` and `hashBlockCommitments` are stored as committed by the proof
  of work but not checked against block data. Proving facts about them (an Ironwood root, a transaction) is the job of
  the inclusion proofs designed in docs/TIER2.md.
- **The checkpoint.** Its hash, height and the 113 context entries (times and nBits of heights checkpoint − 112 ..
  checkpoint) are trusted constructor input.
- **Mainnet.** Testnet parameters only (PoWLimit, minimum-difficulty rule, 653,606 / 4,465,026). Mainnet would need
  its PoWLimit (2^243 − 1), no minimum-difficulty rule, its NU7 height and max-time start height 2.
- **Finality.** No checkpoint finality or reorg depth limit beyond the most-work rule.

**Testnet caveat.** The minimum-difficulty rule lets anyone mine a block at difficulty 1 when its time is more than
450 s after its parent's. Such a block adds 32 units of work, a normal post-NU7 block about 380,000. The 2-hour
clock rule bounds how far an attacker can push timestamps, but blocks of that kind can still extend the best chain
cheaply, and once the averaging window holds such blocks the following ones stay easy too: in a simulation, 300
extra confirmations cost 0.085 of one honest block's work. `confirmations` counts them as blocks;
`confirmationWork` does not inflate. A consumer that needs reorg resistance on Testnet must require work, never a
block count (docs/TIER2.md).

**Reorganisations.** A reorganisation rewrites the best-chain index of the new branch only; heights above the new
tip are left as stale entries that every view ignores (all reads are bounded by the tip height). Clearing them used
to cost gas per orphaned height, so a long, light branch built on a stale tip could push the honest reorganisation
above the transaction gas limit and freeze the relay (contracts review R-1).

## Gas

Measured with `forge test --isolate` (each submission its own transaction, cold storage), solc 0.8.28, optimizer 200
runs, over the 32 real post-NU7 headers of `testnet_recent.json`:

| | gas |
|---|---|
| `submitHeader`, extending the best chain: whole transaction (21,000 + 23,796 calldata for 1,572 bytes + execution) | min 613,612 · **mean 625,505** · max 664,546 |
| execution of one typical header (trace of header 4,488,005) | 576,345 |
| ↳ Equihash | 361,100: 512 index decodes with duplicate check and 513 BLAKE2b compressions (274,120), tree check (82,843) |
| ↳ time rules and difficulty adjustment | 76,463: mean target over 102 blocks (56,766, mostly 13 cold storage reads), two medians (≈15,400) |
| ↳ storage (4-slot record, best-chain indexes, tip) and events | ≈ 134,000 |
| fork test, whole transactions: minimum-difficulty blocks (no mean target to compute), side branches, reorganisations of up to 8 blocks | 517,124 – 656,667 |
| deployment, including the 113 context entries | 2,881,672 |

`forge test --isolate --match-path 'test/relay/*' --match-test test_gas -vv` prints these per-call numbers. At 25-second blocks,
relaying every header is ~3,456 transactions per day (≈ 2.2 × 10^9 gas). Proving batches of headers in a zkVM
instead is discussed in docs/TIER2.md.

## Tests

```bash
cd contracts
forge test --match-path 'test/relay/*'            # 44 tests, no RPC, about 1 s
forge test --isolate --match-path 'test/relay/*'  # same, every top-level call its own transaction
```

- `ZcashHeaderRelay.t.sol` (27): the 32 recent post-NU7 headers and the 30 headers across the NU7 activation, each
  submitted and checked (tip, height, confirmations, stored fields); batch submission; rejections: unknown parent,
  duplicate, length and solution size, version, time ≤ median-time-past, time > median + 90 min, time > clock + 2 h,
  wrong nBits, the minimum-difficulty rule both ways (a 451 s gap requires `0x2007ffff`; a block 30 s or exactly 450 s
  after its parent with a genuine solution at `0x2007ffff` is rejected), Equihash bit flips in solution, nonce and
  header, swapped leaves and subtrees, duplicated indices, a valid solution whose hash is above the target; fork
  choice: a lighter fork does not reorganise, an equal-work fork keeps the first tip, a heavier fork (8 blocks over 7)
  reorganises and the real block then reorganises back; a side-branch block is checked against its own branch's
  history; constructor input validation; gas.
- `ZcashEquihash.t.sol` (9): the hard-coded BLAKE2b initial state recomputed from the IV and parameter block; the
  EIP-152 precompile against its test vector 5; leaf strings against Python's `hashlib.blake2b`; all 62 real solutions;
  the mutations with the reference's error kinds; crafted trees for every error kind (including `NonZeroRootHash`); a
  fuzzed bit flip anywhere in a real solution.
- `ZcashPow.t.sol` (8): nBits decoding and encoding against zcashd `SetCompact`/`GetCompact` vectors, PoWLimit, work,
  damping truncation, clamps and cap, median, byte reversal, and a fuzzed compact round trip.

`foundry.toml` gained one read-only `fs_permissions` entry for `./test/relay/fixtures`.

## Fixtures

| File | Content | Source |
|---|---|---|
| `testnet_recent.json` | checkpoint 4,487,999 + 113 context entries; 32 headers 4,488,000 .. 4,488,031 | Zcash Testnet P2P |
| `testnet_nu7_activation.json` | checkpoint 4,465,014 + context; 30 headers 4,465,015 .. 4,465,044 (11 pre-NU7, 19 post-NU7) | Zcash Testnet P2P |
| `testnet_synthetic.json` | **SYNTHETIC**: one-defect mutations of header 4,488,000; fork headers on block 4,488,002 | generated |

Raw headers come from the Zcash Testnet P2P network itself (`getheaders` from a `/Zebra:7.0.0-rc.0/` peer found
through the Testnet DNS seeders, protocol 170180): the project's light wallet server leaves `CompactBlock.header`
empty and no public explorer exposes `hashBlockCommitments`. `tools/zcash_p2p_headers.py` recomputes every SHA256d
and checks the linkage while fetching. `tools/build_fixture.py --cipherscan` compared every fixture entry with the
independent CipherScan Testnet API (hash, previous hash, time, nBits; for full headers also version, merkle root, nonce
and solution): all matched on 2026-10-11. `tools/zcash_header_check.py fixture <file>` re-verifies a fixture offline.

The synthetic fork headers are not Zcash blocks, but their Equihash solutions are genuine: they were found at the
Testnet minimum difficulty by Tromp's solver (`equihash` 0.3.0, `solver` feature) through
`research/relay-synthetic-headers`, which keeps only solutions the reference verifier accepts (the solver sometimes
returns candidates with duplicate indices). `tools/make_synthetic.py` re-verifies each one and records the expected
nBits and median-time-past from the Python model.

Regenerating (network access for the first two steps; no step is needed to run the tests):

```bash
cd contracts/test/relay/tools
python3 -I zcash_p2p_headers.py --from-hash <display hash of block h-1> --from-height <h-1> --count N --out all.jsonl
python3 -I build_fixture.py --headers all.jsonl --peers all.jsonl.peers --checkpoint-height H --count 32 \
    --description "..." --out ../fixtures/testnet_recent.json --cipherscan
(cd ../../../../research/relay-synthetic-headers && cargo build --release)
python3 -I make_synthetic.py ../../../../research/relay-synthetic-headers/target/release/relay-synthetic-headers ../fixtures
python3 -I zcash_header_check.py fixture ../fixtures/testnet_recent.json
```

Nothing here is a mock: the relay verifies real proof of work, and the synthetic data exists only in tests.
