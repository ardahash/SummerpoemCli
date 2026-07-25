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
    /// Known genesis nonce. When set, the genesis block is verified (one cheap
    /// hash) rather than re-mined, so first launch is fast. `None` means mine
    /// it (used on regtest, whose easy target is found in a few hashes).
    pub genesis_nonce: Option<u64>,
    pub address_hrp: &'static str,
    /// Default seed nodes for peer discovery (host:port). Non-empty on
    /// mainnet; empty on regtest.
    pub seeds: &'static [&'static str],
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
            genesis_time: 1_784_851_200, // 2026-07-24T00:00:00Z
            genesis_message: "What if life was meant to be lived",
            genesis_nonce: Some(11_110_300), // verified; hash 60235b42...1ca0d
            address_hrp: "sump",
            // Public bootstrap seed. This hostname must be kept online for
            // release builds; nodes learn more peers through address gossip.
            seeds: &["seed.summerpoem.org:8776", "46.62.224.182:8776"],
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
            genesis_nonce: None, // easy target — mine it each time
            address_hrp: "sumprt",
            seeds: &[],
        }
    }

    pub fn for_network(n: Network) -> Params {
        match n {
            Network::Mainnet => Params::mainnet(),
            Network::Regtest => Params::regtest(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mainnet_has_public_bootstrap_seed() {
        let params = Params::mainnet();
        assert!(!params.seeds.is_empty(), "mainnet needs a public seed");
        for seed in params.seeds {
            let (host, port) = seed.rsplit_once(':').expect("seed must be host:port");
            assert!(!host.is_empty());
            assert_eq!(port, "8776");
            assert_ne!(host, "localhost");
            assert!(!host.starts_with("127."));
            assert_ne!(host, "0.0.0.0");
        }
    }
}
