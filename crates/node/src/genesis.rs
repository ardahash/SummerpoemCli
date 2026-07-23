//! Deterministic genesis block construction.

use sump_core::block::{Block, BlockHeader};
use sump_core::compact::{bits_to_target, target_to_bits};
use sump_core::emission::block_reward;
use sump_core::hash::Hash256;
use sump_core::params::Params;
use sump_core::tx::{Lock, Transaction, TxBody, TxOutput};
use sump_pow::PowContext;

/// Build and mine the genesis block. The genesis coinbase pays the height-0
/// subsidy to the all-zero key hash — provably out of anyone's control
/// (fair launch: not even the founders can spend it).
pub fn build_genesis(params: &Params, ctx: &PowContext) -> Block {
    let mut coinbase_data = 0u64.to_le_bytes().to_vec();
    coinbase_data.extend_from_slice(params.genesis_message.as_bytes());
    let coinbase = Transaction {
        body: TxBody {
            version: 1,
            inputs: vec![],
            outputs: vec![TxOutput {
                amount: block_reward(0),
                lock: Lock::P2pkh { pkh: [0u8; 20] },
            }],
            locktime: 0,
            coinbase_data,
        },
        witnesses: vec![],
    };
    let mut block = Block {
        header: BlockHeader {
            version: 1,
            prev: Hash256::ZERO,
            tx_root: Hash256::ZERO,
            witness_root: Hash256::ZERO,
            time: params.genesis_time,
            bits: target_to_bits(params.pow_limit),
            nonce: 0,
        },
        transactions: vec![coinbase],
    };
    block.header.tx_root = block.compute_tx_root();
    block.header.witness_root = block.compute_witness_root();

    let target = bits_to_target(block.header.bits).expect("valid bits");
    let msg = block.header.pow_message();
    let nonce = sump_pow::mine(ctx, &msg, target, 0, u64::MAX).expect("genesis mine");
    block.header.nonce = nonce;
    block
}
