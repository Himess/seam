//! Zcash block headers from the peer-to-peer network of Testnet or Mainnet: the version handshake, `getheaders` and
//! `headers`. Read-only: nothing is ever relayed. A port of Seam's `contracts/test/relay/tools/zcash_p2p_headers.py`
//! by way of Zecured's `signer/crates/deposit-witness/src/p2p.rs` (73de9f909e270e8aea10c2b221905f62a6a4c4df).
//!
//! A light wallet server's compact blocks carry no full header, and a history-tree leaf needs every block's nBits, so
//! the witness builder needs the headers of the whole epoch. Nothing here has to be trusted: every header's encoding,
//! SHA-256d hash and link to the one before are checked, the history root built from them must match the anchor
//! header's commitments, and the anchor itself is checked on the EVM chain by the relay.
//!
//! Peer addresses are never printed, logged or written: they are the caller's to keep local.

use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream, ToSocketAddrs},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, ensure};
use rand_core::{OsRng, RngCore};

use crate::{
    history::{HEADER_LENGTH, sha256d},
    network::ZcashNetwork,
};

/// A Zcash network's message magic, default port, DNS seeders and the protocol version announced to its peers.
pub struct NetworkParams {
    pub magic: [u8; 4],
    pub port: u16,
    pub seeders: &'static [&'static str],
    pub protocol_version: i32,
}

/// From `zcash_p2p_headers.py`: the magics, ports and seeders of Zebra v7.0.0-rc.0 (`zebra-chain/src/parameters/
/// constants.rs` `magics`, `network.rs` `default_port`, `zebra-network/src/config.rs` `DEFAULT_MAINNET_PEERS`; for
/// Testnet the two seeders the tool always used). Each network announces ZIP 259's minimum protocol version for NU7:
/// 170,180 on Testnet and 170,190 on Mainnet, where peers drop lower versions once NU7 activates (Zebra 7.0.0's
/// `min_specified_for_upgrade`).
pub fn network_params(network: ZcashNetwork) -> NetworkParams {
    match network {
        ZcashNetwork::Testnet => NetworkParams {
            magic: [0xfa, 0x1a, 0xf9, 0xbf],
            port: 18_233,
            seeders: &["dnsseed.testnet.z.cash", "testnet.seeder.zfnd.org"],
            protocol_version: 170_180,
        },
        ZcashNetwork::Mainnet => NetworkParams {
            magic: [0x24, 0xe9, 0x27, 0x64],
            port: 8_233,
            seeders: &[
                "dnsseed.z.cash",
                "dnsseed.str4d.xyz",
                "mainnet.seeder.zfnd.org",
                "mainnet.seeder.shieldedinfra.net",
                "seeder.zec.rocks",
            ],
            protocol_version: 170_190,
        },
    }
}

const USER_AGENT: &[u8] = concat!("/seam-chain:", env!("CARGO_PKG_VERSION"), "/").as_bytes();
const PREFIX_LENGTH: usize = 140;
const SOLUTION_LENGTH: usize = 1344;
const MAX_MESSAGE: usize = 8_000_000;
/// Many seeded addresses do not answer: give up on a connection quickly, on a reply slowly.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const TIMEOUT: Duration = Duration::from_secs(30);
/// A peer that keeps the connection alive (pings, inventory) but never answers `getheaders` must not hold the caller
/// forever (a responsive peer answers in about a second).
const HEADERS_DEADLINE: Duration = Duration::from_secs(15);
/// Most headers one `headers` message may carry (the protocol's limit).
const MAX_HEADERS: u64 = 2_000;
/// Most headers zcashd and Zebra send in one answer (`MAX_HEADERS_RESULTS`, `MAX_FIND_BLOCK_HEADERS_RESULTS`): a
/// shorter answer ends at the peer's tip.
pub const PEER_HEADERS_PER_ANSWER: usize = 160;

pub(crate) fn varint(n: u64) -> Vec<u8> {
    match n {
        0..0xfd => vec![n as u8],
        0xfd..=0xffff => [&[0xfd][..], &(n as u16).to_le_bytes()].concat(),
        0x1_0000..=0xffff_ffff => [&[0xfe][..], &(n as u32).to_le_bytes()].concat(),
        _ => [&[0xff][..], &n.to_le_bytes()].concat(),
    }
}

fn read_varint(buf: &[u8], pos: usize) -> Result<(u64, usize)> {
    let first = *buf.get(pos).context("truncated compactSize")?;
    let take = |n: usize| -> Result<&[u8]> { buf.get(pos + 1..pos + 1 + n).context("truncated compactSize") };
    Ok(match first {
        0xfd => (u64::from(u16::from_le_bytes(take(2)?.try_into()?)), pos + 3),
        0xfe => (u64::from(u32::from_le_bytes(take(4)?.try_into()?)), pos + 5),
        0xff => (u64::from_le_bytes(take(8)?.try_into()?), pos + 9),
        b => (u64::from(b), pos + 1),
    })
}

fn net_addr(ip: Option<std::net::Ipv4Addr>, port: u16) -> Vec<u8> {
    let mut out = 0u64.to_le_bytes().to_vec(); // services
    out.extend_from_slice(&[0u8; 10]);
    out.extend_from_slice(&[0xff, 0xff]);
    out.extend_from_slice(&ip.map_or([0; 4], |a| a.octets()));
    out.extend_from_slice(&port.to_be_bytes());
    out
}

/// A message: magic, command, payload length, checksum (the first 4 bytes of the payload's SHA-256d), payload.
fn message(magic: [u8; 4], command: &str, payload: &[u8]) -> Result<Vec<u8>> {
    ensure!(command.len() <= 12, "command too long");
    let mut name = [0u8; 12];
    name[..command.len()].copy_from_slice(command.as_bytes());
    let mut msg = magic.to_vec();
    msg.extend_from_slice(&name);
    msg.extend_from_slice(&u32::try_from(payload.len())?.to_le_bytes());
    msg.extend_from_slice(&sha256d(payload)[..4]);
    msg.extend_from_slice(payload);
    Ok(msg)
}

/// The `version` payload announcing `network`'s protocol version to the peer at `addr`, with no services and no
/// relaying.
fn version_payload(network: ZcashNetwork, addr: SocketAddr, now: i64, nonce: u64) -> Vec<u8> {
    let ip = match addr {
        SocketAddr::V4(a) => Some(*a.ip()),
        SocketAddr::V6(_) => None,
    };
    let mut payload = network_params(network).protocol_version.to_le_bytes().to_vec();
    payload.extend_from_slice(&0u64.to_le_bytes()); // services
    payload.extend_from_slice(&now.to_le_bytes());
    payload.extend_from_slice(&net_addr(ip, addr.port()));
    payload.extend_from_slice(&net_addr(None, 0));
    payload.extend_from_slice(&nonce.to_le_bytes());
    payload.extend_from_slice(&varint(USER_AGENT.len() as u64));
    payload.extend_from_slice(USER_AGENT);
    payload.extend_from_slice(&0i32.to_le_bytes()); // start height
    payload.push(0); // relay = false
    payload
}

/// The `getheaders` payload: `network`'s protocol version, the locator (internal byte order, newest first) and no
/// stop hash. A peer answers with the headers after the newest locator hash on its best chain.
fn getheaders_payload(network: ZcashNetwork, locator: &[[u8; 32]]) -> Vec<u8> {
    let mut payload = network_params(network).protocol_version.to_le_bytes().to_vec();
    payload.extend_from_slice(&varint(locator.len() as u64));
    for hash in locator {
        payload.extend_from_slice(hash);
    }
    payload.extend_from_slice(&[0u8; 32]);
    payload
}

/// A connection to one peer of a Zcash network.
pub struct Peer {
    stream: TcpStream,
    network: ZcashNetwork,
    /// The network's message magic: a peer of another network is refused at its first message.
    magic: [u8; 4],
    buf: Vec<u8>,
    /// The best height the peer announced in its `version` message (its word only).
    pub start_height: u32,
}

impl Peer {
    /// Connects to `addr`, a peer of Zcash `network`, and completes the version handshake.
    pub fn connect(network: ZcashNetwork, addr: SocketAddr) -> Result<Self> {
        let stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT).context("connect to a peer")?;
        stream.set_read_timeout(Some(TIMEOUT))?;
        stream.set_write_timeout(Some(TIMEOUT))?;
        let mut peer = Peer {
            stream,
            network,
            magic: network_params(network).magic,
            buf: vec![],
            start_height: 0,
        };
        peer.handshake(addr)?;
        Ok(peer)
    }

    fn send(&mut self, command: &str, payload: &[u8]) -> Result<()> {
        let msg = message(self.magic, command, payload)?;
        self.stream.write_all(&msg).context("send to a peer")
    }

    fn recv(&mut self) -> Result<(String, Vec<u8>)> {
        loop {
            if self.buf.len() >= 24 {
                ensure!(self.buf[..4] == self.magic, "a peer of another network");
                let length = u32::from_le_bytes(self.buf[16..20].try_into()?) as usize;
                ensure!(length <= MAX_MESSAGE, "an oversized message");
                if self.buf.len() >= 24 + length {
                    let command = String::from_utf8_lossy(&self.buf[4..16]).trim_end_matches('\0').to_owned();
                    let checksum: [u8; 4] = self.buf[20..24].try_into()?;
                    let payload = self.buf[24..24 + length].to_vec();
                    self.buf.drain(..24 + length);
                    ensure!(sha256d(&payload)[..4] == checksum, "a bad message checksum");
                    return Ok((command, payload));
                }
            }
            let mut chunk = [0u8; 1 << 16];
            let n = self.stream.read(&mut chunk).context("receive from a peer")?;
            ensure!(n > 0, "the peer closed the connection");
            self.buf.extend_from_slice(&chunk[..n]);
        }
    }

    fn handshake(&mut self, addr: SocketAddr) -> Result<()> {
        let now = i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())?;
        self.send("version", &version_payload(self.network, addr, now, OsRng.next_u64()))?;
        let (mut version, mut verack) = (false, false);
        while !(version && verack) {
            let (command, body) = self.recv()?;
            match command.as_str() {
                "version" => {
                    // version, services, timestamp, two addresses and the nonce come first: 80 bytes.
                    let (len, pos) = read_varint(&body, 80)?;
                    let end = pos.checked_add(usize::try_from(len)?).context("a truncated version message")?;
                    let height = end
                        .checked_add(4)
                        .and_then(|after| body.get(end..after))
                        .context("a truncated version message")?;
                    self.start_height = u32::try_from(i32::from_le_bytes(height.try_into()?)).unwrap_or(0);
                    version = true;
                    self.send("verack", &[])?;
                }
                "verack" => verack = true,
                "ping" => self.send("pong", &body)?,
                _ => {}
            }
        }
        Ok(())
    }

    /// The headers a peer sends after the newest hash of `locator` (internal byte order, newest first) on its best
    /// chain, up to 2,000.
    pub fn get_headers(&mut self, locator: &[[u8; 32]]) -> Result<Vec<Vec<u8>>> {
        self.send("getheaders", &getheaders_payload(self.network, locator))?;
        let started = Instant::now();
        loop {
            ensure!(started.elapsed() < HEADERS_DEADLINE, "no headers from the peer in time");
            let (command, body) = self.recv()?;
            match command.as_str() {
                "headers" => return parse_headers(&body),
                "ping" => self.send("pong", &body)?,
                _ => {}
            }
        }
    }
}

/// The 1487-byte headers of a `headers` message (each followed by a zero transaction count).
pub(crate) fn parse_headers(body: &[u8]) -> Result<Vec<Vec<u8>>> {
    let (count, mut pos) = read_varint(body, 0)?;
    // The count is the peer's word: never allocate for more headers than the protocol allows or than the message can
    // hold.
    ensure!(count <= MAX_HEADERS, "a headers message claims {count} headers");
    ensure!(count as usize <= body.len() / HEADER_LENGTH, "a headers message too short for {count} headers");
    let mut out = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let (solution, at) = read_varint(body, pos + PREFIX_LENGTH)?;
        ensure!(
            solution as usize == SOLUTION_LENGTH && at == pos + PREFIX_LENGTH + 3,
            "an unexpected solution encoding"
        );
        let raw = body.get(pos..at + SOLUTION_LENGTH).context("a truncated header")?;
        ensure!(raw.len() == HEADER_LENGTH, "a truncated header");
        out.push(raw.to_vec());
        let (transactions, next) = read_varint(body, at + SOLUTION_LENGTH)?;
        ensure!(transactions == 0, "a headers message with transactions");
        pos = next;
    }
    ensure!(pos == body.len(), "trailing bytes in a headers message");
    Ok(out)
}

/// Peers of Zcash `network` from its DNS seeders (IPv4), in random order.
pub fn seed_peers(network: ZcashNetwork) -> Vec<SocketAddr> {
    let params = network_params(network);
    let mut out: Vec<SocketAddr> = vec![];
    for seeder in params.seeders {
        if let Ok(addrs) = (*seeder, params.port).to_socket_addrs() {
            for a in addrs.filter(SocketAddr::is_ipv4) {
                if !out.contains(&a) {
                    out.push(a);
                }
            }
        }
    }
    // Fisher-Yates: spread the load over the seeded peers.
    for i in (1..out.len()).rev() {
        let j = (OsRng.next_u64() % (i as u64 + 1)) as usize;
        out.swap(i, j);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_sizes_round_trip() {
        for n in [0u64, 1, 0xfc, 0xfd, 0xffff, 0x1_0000, 0xffff_ffff, 0x1_0000_0000] {
            let enc = varint(n);
            assert_eq!(read_varint(&enc, 0).unwrap(), (n, enc.len()), "{n}");
        }
        assert!(read_varint(&[0xfd, 1], 0).is_err());
    }

    /// Each network announces ZIP 259's NU7 protocol version, in `version` and in `getheaders`, under its own magic.
    #[test]
    fn each_network_announces_its_protocol_version() {
        let addr: SocketAddr = "127.0.0.1:18233".parse().unwrap();
        for (network, version, magic) in [
            (ZcashNetwork::Testnet, 170_180i32, [0xfa, 0x1a, 0xf9, 0xbf]),
            (ZcashNetwork::Mainnet, 170_190, [0x24, 0xe9, 0x27, 0x64]),
        ] {
            let v = version_payload(network, addr, 1_700_000_000, 7);
            assert_eq!(&v[..4], &version.to_le_bytes());
            assert_eq!(&v[12..20], &1_700_000_000i64.to_le_bytes());
            // The receiver's address: services, then an IPv4-mapped IPv6 address and the port (big-endian).
            assert_eq!(&v[38..40], &[0xff, 0xff]);
            assert_eq!(&v[40..44], &[127, 0, 0, 1]);
            assert_eq!(&v[44..46], &18_233u16.to_be_bytes());
            assert_eq!(&v[72..80], &7u64.to_le_bytes(), "the nonce");
            assert_eq!(usize::from(v[80]), USER_AGENT.len());
            assert_eq!(&v[81..81 + USER_AGENT.len()], USER_AGENT);
            assert_eq!(v.len(), 81 + USER_AGENT.len() + 5);
            let g = getheaders_payload(network, &[[1; 32], [2; 32]]);
            assert_eq!(&g[..4], &version.to_le_bytes());
            assert_eq!(g[4], 2);
            assert_eq!(g.len(), 4 + 1 + 3 * 32);
            let m = message(network_params(network).magic, "getheaders", &g).unwrap();
            assert_eq!(&m[..4], &magic);
            assert_eq!(&m[4..16], b"getheaders\0\0");
            assert_eq!(&m[16..20], &(g.len() as u32).to_le_bytes());
            assert_eq!(&m[20..24], &sha256d(&g)[..4]);
            assert_eq!(&m[24..], &g[..]);
        }
        assert!(message([0; 4], "thirteen-long", &[]).is_err());
    }

    /// A peer's claimed header count must not size an allocation.
    #[test]
    fn a_claimed_header_count_is_checked_before_allocating() {
        let huge = [&[0xff][..], &u64::MAX.to_le_bytes()].concat();
        assert!(parse_headers(&huge).is_err());
        let too_many = [&[0xfd][..], &2_001u16.to_le_bytes()].concat();
        assert!(parse_headers(&too_many).is_err());
        let empty = [&[0xfd][..], &2_000u16.to_le_bytes()].concat();
        assert!(parse_headers(&empty).is_err());
        assert_eq!(parse_headers(&[0]).unwrap(), Vec::<Vec<u8>>::new());
    }

    #[test]
    fn headers_messages_are_checked() {
        let header = vec![0u8; PREFIX_LENGTH]
            .into_iter()
            .chain([0xfd, 0x40, 0x05])
            .chain(vec![7u8; SOLUTION_LENGTH])
            .collect::<Vec<u8>>();
        let mut body = varint(2);
        for _ in 0..2 {
            body.extend_from_slice(&header);
            body.push(0);
        }
        assert_eq!(parse_headers(&body).unwrap(), vec![header.clone(), header.clone()]);
        let mut with_tx = body.clone();
        with_tx[1 + HEADER_LENGTH] = 1;
        assert!(parse_headers(&with_tx).is_err());
        assert!(parse_headers(&body[..body.len() - 1]).is_err());
        let mut trailing = body.clone();
        trailing.push(0);
        assert!(parse_headers(&trailing).is_err());
    }
}
