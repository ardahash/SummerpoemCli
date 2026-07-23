//! Network parameters.

use primitive_types::U256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Network {
    Mainnet,
    Regtest,
}

impl Network {
    pub fn id(&self) -> u8 {
        match self {
            Network::Mainnet => 0,
            Network::Regtest => 1,
        }
    }

    pub fn from_id(id: u8) -> Option<Network> {
        match id {
            0 => Some(Network::Mainnet),
            1 => Some(Network::Regtest),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PowParams {
    pub cache_bytes: usize,
    pub dataset_bytes: usize,
    pub epoch_length: u64,
    pub accesses: usize,
    pub parents: usize,
}

#[derive(Clone, Debug)]
pub struct Params {
    pub network: Network,
    /// Target seconds between blocks.
    pub block_interval: u64,
    /// ASERT time constant in seconds.
    pub asert_tau: u64,
    /// Easiest permitted target; also the genesis/anchor target.
    pub pow_limit: U256,
    pub coinbase_maturity: u64,
    pub max_block_size: usize,
    pub pow: PowParams,
    pub genesis_time: u64,
    pub genesis_message: &'static str,
    pub address_hrp: &'static str,
}

impl Params {
    pub fn mainnet() -> Params {
        Params {
            network: Network::Mainnet,
            block_interval: 60,
            asert_tau: 86_400,
            pow_limit: U256::MAX >> 24,
            coinbase_maturity: 100,
            max_block_size: 4_000_000,
            pow: PowParams {
                cache_bytes: 64 << 20,
                dataset_bytes: 2 << 30,
                epoch_length: 4096,
                accesses: 64,
                parents: 64,
            },
            genesis_time: 1_782_864_000, // 2026-07-01T00:00:00Z
            genesis_message: "Summerpoem genesis: the chain that never assumed quantum computers can't exist.",
            address_hrp: "sump",
        }
    }

    pub fn regtest() -> Params {
        Params {
            network: Network::Regtest,
            block_interval: 60,
            asert_tau: 3_600,
            pow_limit: U256::MAX >> 4,
            coinbase_maturity: 5,
            max_block_size: 4_000_000,
            pow: PowParams {
                cache_bytes: 16 << 10,
                dataset_bytes: 256 << 10,
                epoch_length: 64,
                accesses: 16,
                parents: 16,
            },
            genesis_time: 1_782_864_000,
            genesis_message: "Summerpoem regtest genesis.",
            address_hrp: "sumprt",
        }
    }

    pub fn for_network(n: Network) -> Params {
        match n {
            Network::Mainnet => Params::mainnet(),
            Network::Regtest => Params::regtest(),
        }
    }
}
