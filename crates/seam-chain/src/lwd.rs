//! A light wallet server (lightwalletd, or Zaino) over gRPC: transactions, compact blocks and note commitment tree
//! states. Ported from Zecured's `signer/crates/zcash-ops/src/lwd.rs` (73de9f909e270e8aea10c2b221905f62a6a4c4df),
//! with zcash_client_backend's generated client as there.
//!
//! For a witness, the server is trusted for data availability only: every compact block is checked against its header
//! from the P2P network, every rebuilt note commitment tree against the server's own tree states, and the history root
//! against the anchor header's commitments. The headers' proof of work is left to the relay, which must hold the anchor.
//! A wallet scan for `seam pay` checks nothing against headers: there the server can hide notes, or hold back a
//! transaction it says it accepted. Every call is bounded in time. A server's URL may carry an API key, so it never
//! appears in an error or a log.

use std::time::Duration;

use anyhow::{Context, Result, anyhow, ensure};
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};
use zcash_client_backend::proto::{
    compact_formats::CompactBlock,
    service::{
        self, BlockId, BlockRange, ChainSpec, Empty, RawTransaction, TreeState, TxFilter,
        compact_tx_streamer_client::CompactTxStreamerClient,
    },
};
use zcash_protocol::TxId;

use crate::network::ZcashNetwork;

/// A gRPC client for the CompactTxStreamer service.
pub type LwdClient = CompactTxStreamerClient<Channel>;

/// Time allowed to establish a connection (TCP and TLS).
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Time allowed for a call to answer (for a stream: until it starts).
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// Longest wait for the next message of a streamed answer.
pub const STREAM_MESSAGE_TIMEOUT: Duration = Duration::from_secs(60);

/// Pool types to request: with all four, a compact block lists every transaction of its block (with only the
/// shielded ones, transparent-only transactions such as most coinbases are left out, and neither the txid tree nor
/// the auth data tree could be rebuilt).
pub const ALL_POOLS: [service::PoolType; 4] = [
    service::PoolType::Transparent,
    service::PoolType::Sapling,
    service::PoolType::Orchard,
    service::PoolType::Ironwood,
];

/// Connects to a light wallet server, e.g. `https://testnet.zec.rocks:443`.
pub async fn connect(url: &str) -> Result<LwdClient> {
    let mut endpoint = Endpoint::from_shared(url.to_owned())
        .map_err(|_| anyhow!("the light wallet server's URL is not valid"))?
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT);
    if url.starts_with("https://") {
        endpoint = endpoint.tls_config(ClientTlsConfig::new().with_webpki_roots()).context("TLS configuration")?;
    }
    let channel = endpoint.connect().await.map_err(|e| anyhow!("connect to the light wallet server: {e}"))?;
    // Compact block ranges can be large: allow bigger messages than tonic's 4 MiB default.
    const MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;
    Ok(CompactTxStreamerClient::new(channel).max_decoding_message_size(MAX_MESSAGE_BYTES))
}

/// The server's identity and chain tip, as it reports them.
pub async fn lightd_info(client: &mut LwdClient) -> Result<service::LightdInfo> {
    Ok(client
        .get_lightd_info(Empty {})
        .await
        .map_err(|s| anyhow!("GetLightdInfo: {}", s.message()))?
        .into_inner())
}

/// That the server follows Zcash `network`: a server of another network is refused before anything is read from it.
pub async fn check_network(client: &mut LwdClient, network: ZcashNetwork) -> Result<()> {
    let chain = lightd_info(client).await?.chain_name;
    ensure!(
        chain == network.lightd_chain_name(),
        "the light wallet server follows Zcash chain {chain:?}, not Zcash {network}"
    );
    Ok(())
}

/// The height and hash of the latest block the server knows.
pub async fn latest_block(client: &mut LwdClient) -> Result<BlockId> {
    Ok(client
        .get_latest_block(ChainSpec {})
        .await
        .map_err(|s| anyhow!("GetLatestBlock: {}", s.message()))?
        .into_inner())
}

fn block_id(height: u32) -> BlockId {
    BlockId {
        height: height.into(),
        hash: vec![],
    }
}

/// The raw bytes of a transaction and the height of the block that mined it (0 if it is not mined).
pub async fn raw_transaction(client: &mut LwdClient, txid: &TxId) -> Result<(Vec<u8>, u64)> {
    let raw = client
        .get_transaction(TxFilter {
            hash: txid.as_ref().to_vec(),
            ..Default::default()
        })
        .await
        // The server's message may name the transaction: the status code is enough.
        .map_err(|s| anyhow!("GetTransaction failed ({:?})", s.code()))?
        .into_inner();
    Ok((raw.data, raw.height))
}

/// The compact blocks `from..=to`, every pool type requested, each checked to be at its height.
pub async fn compact_blocks(client: &mut LwdClient, from: u32, to: u32) -> Result<Vec<CompactBlock>> {
    ensure!(from <= to, "no blocks to fetch");
    let mut stream = client
        .get_block_range(BlockRange {
            start: Some(block_id(from)),
            end: Some(block_id(to)),
            pool_types: ALL_POOLS.iter().map(|p| *p as i32).collect(),
        })
        .await
        .map_err(|s| anyhow!("GetBlockRange({from}..={to}): {}", s.message()))?
        .into_inner();
    let mut blocks = Vec::with_capacity((to - from + 1) as usize);
    while let Some(b) = tokio::time::timeout(STREAM_MESSAGE_TIMEOUT, stream.message())
        .await
        .context("GetBlockRange: the stream stalled")?
        .map_err(|s| anyhow!("GetBlockRange: {}", s.message()))?
    {
        let expected = from + blocks.len() as u32;
        ensure!(b.height == u64::from(expected), "the server sent block {} for {expected}", b.height);
        blocks.push(b);
    }
    ensure!(
        blocks.len() == (to - from + 1) as usize,
        "the server sent {} blocks for {from}..={to}",
        blocks.len()
    );
    Ok(blocks)
}

/// The server's note commitment tree states after block `height`.
pub async fn tree_state(client: &mut LwdClient, height: u32) -> Result<TreeState> {
    let state = client
        .get_tree_state(block_id(height))
        .await
        .map_err(|s| anyhow!("GetTreeState({height}): {}", s.message()))?
        .into_inner();
    ensure!(state.height == u64::from(height), "the server sent the tree state of {} for {height}", state.height);
    Ok(state)
}

/// Sends a serialized transaction through the server.
pub async fn send_transaction(client: &mut LwdClient, data: Vec<u8>) -> Result<()> {
    let response = client
        .send_transaction(RawTransaction { data, height: 0 })
        .await
        .map_err(|s| anyhow!("SendTransaction failed ({:?})", s.code()))?
        .into_inner();
    ensure!(
        response.error_code == 0,
        "the light wallet server refused the transaction: {} ({})",
        without_txids(&response.error_message),
        response.error_code
    );
    Ok(())
}

/// A server's message with every run of 64 hex digits (a txid, say) replaced by `<hash>`: no txid reaches a log.
pub fn without_txids(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let mut run = String::new();
    for c in message.chars().chain(std::iter::once(' ')) {
        if c.is_ascii_hexdigit() {
            run.push(c);
            continue;
        }
        out.push_str(if run.len() >= 64 { "<hash>" } else { &run });
        run.clear();
        out.push(c);
    }
    out.pop();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_messages_lose_their_txids() {
        let txid = "0123456789abcdef".repeat(4);
        assert_eq!(without_txids(&format!("txn-already-known {txid}")), "txn-already-known <hash>");
        assert_eq!(without_txids(&format!("{txid}:bad")), "<hash>:bad");
        assert_eq!(without_txids("bad-txns-oversize (code 16)"), "bad-txns-oversize (code 16)");
        assert_eq!(without_txids(""), "");
        assert_eq!(without_txids("deadbeef"), "deadbeef");
    }
}
