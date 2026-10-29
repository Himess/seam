//! The Zcash networks Seam reads, and the epochs on each that a payment proof may walk.
//!
//! A ZIP 221 history tree covers one network-upgrade epoch and restarts at the next upgrade, so a payment and its
//! anchor are always in one epoch. Each network's epochs are those its build of the statement allows
//! (`seam_payment::TESTNET`, `seam_payment::MAINNET`), with their activation heights.

use std::fmt;

use anyhow::{Result, bail};
use seam_payment::{NU6_3_BRANCH_ID, NU7_BRANCH_ID};
use serde::{Deserialize, Serialize};
use zcash_protocol::consensus::Network;

/// A Zcash network, as the command line names it (`testnet`, `mainnet`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ZcashNetwork {
    Testnet,
    Mainnet,
}

/// A network-upgrade epoch: its consensus branch id, its first block (the first leaf of its history tree) and the
/// first block of the next epoch, if that is known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Epoch {
    pub branch_id: u32,
    pub activation: u32,
    pub next_activation: Option<u32>,
}

impl Epoch {
    /// Whether block `height` is in this epoch.
    pub fn contains(&self, height: u32) -> bool {
        height >= self.activation && self.next_activation.is_none_or(|next| height < next)
    }
}

/// Testnet: NU7 activated at 4,465,026 (zcash_protocol's height).
const TESTNET_EPOCHS: &[Epoch] = &[Epoch {
    branch_id: NU7_BRANCH_ID,
    activation: 4_465_026,
    next_activation: None,
}];

/// Mainnet: the Ironwood pool exists since NU6.3 at 3,428,143 (zcash_protocol's height); NU7 activates at 3,543,405
/// (ZIP 259 and Zebra 7.0.0, the height `contracts/config/relay-mainnet.json` uses; zcash_protocol =0.11.0-pre.0 has
/// none yet).
const MAINNET_EPOCHS: &[Epoch] = &[
    Epoch {
        branch_id: NU6_3_BRANCH_ID,
        activation: 3_428_143,
        next_activation: Some(3_543_405),
    },
    Epoch {
        branch_id: NU7_BRANCH_ID,
        activation: 3_543_405,
        next_activation: None,
    },
];

impl ZcashNetwork {
    /// The network's name on the command line.
    pub fn from_name(name: &str) -> Result<Self> {
        Ok(match name {
            "testnet" => ZcashNetwork::Testnet,
            "mainnet" => ZcashNetwork::Mainnet,
            other => bail!("unknown Zcash network {other:?} (testnet or mainnet)"),
        })
    }

    /// The consensus parameters (address and key encodings, branch ids by height).
    pub fn params(self) -> Network {
        match self {
            ZcashNetwork::Testnet => Network::TestNetwork,
            ZcashNetwork::Mainnet => Network::MainNetwork,
        }
    }

    /// The statement's parameters for this network: the epochs its build allows.
    pub fn statement(self) -> &'static seam_payment::Network {
        match self {
            ZcashNetwork::Testnet => &seam_payment::TESTNET,
            ZcashNetwork::Mainnet => &seam_payment::MAINNET,
        }
    }

    /// The network a build of the statement proves on (`seam_payment::NETWORK`, set by its `mainnet` feature).
    pub fn of_build() -> Self {
        if seam_payment::NETWORK == seam_payment::MAINNET {
            ZcashNetwork::Mainnet
        } else {
            ZcashNetwork::Testnet
        }
    }

    /// What a light wallet server reports for it (`GetLightdInfo.chain_name`).
    pub fn lightd_chain_name(self) -> &'static str {
        match self {
            ZcashNetwork::Testnet => "test",
            ZcashNetwork::Mainnet => "main",
        }
    }

    /// The epochs a payment proof may walk, oldest first.
    pub fn epochs(self) -> &'static [Epoch] {
        match self {
            ZcashNetwork::Testnet => TESTNET_EPOCHS,
            ZcashNetwork::Mainnet => MAINNET_EPOCHS,
        }
    }

    /// The epoch of block `height`, refused unless a payment proof may walk it.
    pub fn epoch(self, height: u32) -> Result<Epoch> {
        match self.epochs().iter().find(|e| e.contains(height)) {
            Some(e) => Ok(*e),
            None => bail!("block {height} of Zcash {self} is in no epoch a payment proof walks"),
        }
    }
}

impl fmt::Display for ZcashNetwork {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ZcashNetwork::Testnet => "testnet",
            ZcashNetwork::Mainnet => "mainnet",
        })
    }
}

#[cfg(test)]
mod tests {
    use zcash_protocol::consensus::{NetworkUpgrade, Parameters};

    use super::*;

    /// The epochs' heights are zcash_protocol's where it has them; Mainnet's NU7 height is ZIP 259's, which this
    /// release of zcash_protocol does not have yet (compare them again when it does).
    #[test]
    fn the_epochs_are_the_networks() {
        let height = |n: Network, nu| n.activation_height(nu).map(u32::from);
        assert_eq!(height(Network::TestNetwork, NetworkUpgrade::Nu7), Some(TESTNET_EPOCHS[0].activation));
        assert_eq!(height(Network::MainNetwork, NetworkUpgrade::Nu6_3), Some(MAINNET_EPOCHS[0].activation));
        assert_eq!(height(Network::MainNetwork, NetworkUpgrade::Nu7), None);

        // Each epoch is one the network's statement allows, and they follow each other.
        for net in [ZcashNetwork::Testnet, ZcashNetwork::Mainnet] {
            let epochs = net.epochs();
            assert!(epochs.iter().all(|e| net.statement().allows(e.branch_id)), "{net}");
            for pair in epochs.windows(2) {
                assert_eq!(pair[0].next_activation, Some(pair[1].activation), "{net}");
            }
            assert_eq!(epochs.last().unwrap().next_activation, None, "{net}");
        }
    }

    #[test]
    fn the_epoch_of_a_block() {
        let t = ZcashNetwork::Testnet;
        assert_eq!(t.epoch(4_465_026).unwrap().branch_id, NU7_BRANCH_ID);
        assert_eq!(t.epoch(4_488_560).unwrap().activation, 4_465_026);
        // Testnet's NU6.3 epoch is not one the Testnet statement proves.
        assert!(t.epoch(4_465_025).is_err());
        let m = ZcashNetwork::Mainnet;
        assert_eq!(m.epoch(3_428_143).unwrap().branch_id, NU6_3_BRANCH_ID);
        assert_eq!(m.epoch(3_543_404).unwrap().branch_id, NU6_3_BRANCH_ID);
        assert_eq!(m.epoch(3_543_405).unwrap().branch_id, NU7_BRANCH_ID);
        // Mainnet's NU6.2 epoch has no Ironwood pool.
        assert!(m.epoch(3_428_142).is_err());
    }

    #[test]
    fn names_and_parameters() {
        assert_eq!(ZcashNetwork::from_name("testnet").unwrap(), ZcashNetwork::Testnet);
        assert_eq!(ZcashNetwork::from_name("mainnet").unwrap(), ZcashNetwork::Mainnet);
        assert!(ZcashNetwork::from_name("regtest").is_err());
        assert_eq!(ZcashNetwork::Mainnet.to_string(), "mainnet");
        assert_eq!(ZcashNetwork::Testnet.params(), Network::TestNetwork);
        assert_eq!(ZcashNetwork::Testnet.lightd_chain_name(), "test");
        assert_eq!(ZcashNetwork::Mainnet.lightd_chain_name(), "main");
        assert_eq!(*ZcashNetwork::of_build().statement(), seam_payment::NETWORK);
    }
}
