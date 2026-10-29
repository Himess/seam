# Integrating Seam

How a contract reads a Seam relay safely.

## 1. Pick a relay and check what it follows

- **Network.**
  - `networkId()` is `keccak256("zcash-testnet")` or `keccak256("zcash-mainnet")`, available as
    `ZcashNetworks.TESTNET` and `ZcashNetworks.MAINNET`.
  - Check it once, in your constructor: a Testnet relay must never vouch for Mainnet facts.
- **Parameters.** `ZcashHeaderRelay.params()` returns the consensus parameters the relay was deployed with. Compare
  them with [`contracts/config/relay-<network>.json`](../contracts/config).
- **Checkpoint.**
  - The relay trusts its starting block. `bestBlockHashAt(checkpointHeight)` returns the checkpoint hash
    (`checkpointHash()` and `checkpointHeight()` on `ZcashHeaderRelay` too).
  - Compare it with your own node or an explorer, in display order (§ 3).
  - Seam's deployment records, with each checkpoint's context, are in
    [`contracts/deployments/`](../contracts/deployments).

## 2. Install

```sh
forge install Himess/seam@v0.1.0
```

Add the remapping `seam/=lib/seam/contracts/src/`. Then:

- **`seam/interfaces/IZcashHeaderRelayReader.sol`:** the read side, for any 0.8 compiler (CI builds it with solc
  0.8.0). Its selectors and return encodings are the relay's
  ([`contracts/test/examples/IZcashHeaderRelayReader.t.sol`](../contracts/test/examples/IZcashHeaderRelayReader.t.sol)).
- **`seam/relay/IZcashHeaderRelay.sol`** and the relay itself are pinned to solc 0.8.28.
- **TypeScript and other clients:** [`abi/`](../abi).

## 3. Byte order

A block hash is the SHA256d of the 1,487-byte header, in internal byte order. That is the `bytes32` the next header's
`hashPrevBlock` holds, and what the relay takes and returns. Explorers show it byte-reversed.

```sh
python3 -c 'import sys; print("0x" + bytes.fromhex(sys.argv[1])[::-1].hex())' <display-order hash>
```

## 4. Finality: require work, not blocks

- **`confirmationWork(h)`:** the work of the best-chain blocks on top of `h`. It is 0 for the tip itself and 0 for any
  block that isn't on the best chain.
- **`confirmations(h)`:** a block count, `h` included. **Not a security measure on Testnet.**
  - A Testnet block whose time is more than 450 s after its parent's must carry the minimum difficulty, and a miner
    can choose such a time, so block counts are almost free to inflate there. In the relay's simulation, 300 extra
    blocks cost 0.085 of one honest block's work.
  - Mainnet has no such blocks, but work is still the measure to require.

**Choosing `minWork`:**
1. Measure the work of recent blocks.
2. Decide how many blocks deep you want to be.
3. Fix the product at deployment.

Don't compute it on chain from recent blocks: on Testnet those can be cheap.

With Foundry's `cast`, against Seam's relay. `cast` prints each return value on its own line, numbers followed by a
short form in brackets, hence the `sed` and `awk`:

```sh
RELAY=0x77ceDADCFf8bE399C67AB579Ad739820FaC5cE0c
RPC=https://sepolia.base.org
TIP=$(cast call $RELAY "bestChainTip()(bytes32,uint256,uint256)" --rpc-url $RPC | sed -n 2p | awk '{print $1}')
OLD=$(cast call $RELAY "bestBlockHashAt(uint256)(bytes32)" $((TIP - 100)) --rpc-url $RPC)
# The work of the 100 blocks on top of OLD: the tip's chainWork minus OLD's.
W=$(cast call $RELAY "confirmationWork(bytes32)(uint256)" $OLD --rpc-url $RPC | awk '{print $1}')
echo "work per block: $((W / 100))"
```

On 29 October 2026 the 100 blocks up to Zcash Testnet block 4,555,510 added 35,561,993 work: about 355,600 a block,
so 10 blocks ≈ 3,556,000. On Mainnet (`RELAY=0x527B6347d2392d17fa5Bd13DF88A43676501BE48`) the 100 blocks up to 3,535,181 added
259,156,774,611,907: about 2.59 × 10^12 a block.
[`ZcashFinalityGate`](../contracts/src/examples/ZcashFinalityGate.sol) packages the check.

## 5. Reorganisations

- **What happens:** a heavier branch replaces the best chain, and `BestChainUpdated` says so, with `forkHeight` below
  the previous tip's height. `isInBestChain` and `confirmationWork` can then change for blocks above the fork.
- **What to do:** check at the moment you act, and pick a `minWork` that makes reorganising past your action
  uneconomic.
- **Depth:** the relay follows the most work at any depth. Zebra refuses to reorganise below 1,000 blocks.

## 6. A relay that falls behind

The relay knows only the headers it's given. Anyone may feed it and every header is checked, so a feeder can withhold
headers (a delay) but can't forge them. Your app should:
- **Read how far behind it is:** compare `headerOf(tip).time` with `block.timestamp`. This is approximate: a header's
  time can be up to 90 minutes after the median of its predecessors, and the relay allows up to 2 hours ahead of Base's
  clock.
- **Protect users from the lag:** a lagging relay must not hurt a user who did their part on Zcash, for example through
  a deadline that expires against them only because the relay is behind.
- **Feed it when it needs to:** see [RELAYING.md](RELAYING.md).

## 7. What a header tells you

`headerOf(h)` returns:
- `known`, `prevHash`, `height`, `time` and `bits`;
- `chainWork`: cumulative from the checkpoint, which counts 0;
- `merkleRoot` and `blockCommitments`.

The checkpoint itself has no `prevHash`, `merkleRoot` or `blockCommitments` (zero): the relay was given only its hash,
height, time and nBits.

What the last two are:
- **`merkleRoot` (`hashMerkleRoot`)** commits to the block's transaction ids.
- **`blockCommitments` (`hashBlockCommitments`, ZIP 244)** commits to the ZIP 221 chain history root, which commits to
  the Ironwood note commitment tree roots (ZIP 258).
- **Neither is checked against block data:** they are what the proof of work commits to.

To prove something a block contains:
1. Verify a proof whose public output names an anchor block.
2. Check that anchor here (§ 4).
3. Bind the proof's committed values to the anchor's header fields.

## 8. Privacy

Don't put a Zcash transaction id, note commitment or address on a public chain. A Merkle proof of a transaction against
`merkleRoot` links that transaction to whoever submits it. Prove what your app needs in zero knowledge, and reveal only
that. Seam's payment proofs, next on the roadmap, do this for shielded payments.

## 9. Gas

Measured on 29 October 2026 with `forge test --match-path 'test/examples/*' --gas-report` (solc 0.8.28, optimizer 200
runs; the gas report runs every top-level call as its own transaction, so storage is cold). The figures are the view's
own execution; the calling contract adds the call itself (2,600 gas for an address not yet touched in the transaction,
100 after).

| View | Known block | Unknown hash |
|---|---|---|
| `isInBestChain` | 9,079 | 2,589 |
| `confirmationWork` | 9,694 | 2,648 |
| `headerOf` | 9,690 | 3,006 |
| `bestChainTip` | 4,609 | — |
| `bestBlockHashAt` | 6,809 | 4,659 (a height above the tip) |
