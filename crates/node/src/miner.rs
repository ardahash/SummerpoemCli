//! Block template construction and CPU reference mining.

use crate::chain::ChainState;
use crate::error::ValidationError;
use sump_core::block::{Block, BlockHeader};
use sump_core::compact::bits_to_target;
use sump_core::emission::block_reward;
use sump_core::tx::{Lock, Transaction, TxBody, TxOutput};
use sump_pow::PowContext;

/// Build an unmined block template on top of the current tip.
/// `txs` are candidate mempool transactions; invalid or oversize ones are
/// dropped silently.
pub fn build_block_template(
    state: &ChainState,
    txs: &[Transaction],
    payout_pkh: [u8; 20],
    now: u64,
) -> Block {
    let params = state.params();
    let height = state.height() + 1;
    let mtp = state.median_time_past(&state.tip_hash());
    let time = now.max(mtp + 1);

    // greedy inclusion under the size limit
    let mut included: Vec<Transaction> = Vec::new();
    let mut fees: u64 = 0;
    let mut size_budget = params.max_block_size.saturating_sub(4096); // header + coinbase slack
    for tx in txs {
        let Ok(fee) = state.validate_standalone_tx(tx) else {
            continue;
        };
        let sz = tx.size();
        if sz > size_budget {
            continue;
        }
        // conflict with an already-included tx? (same input spent twice)
        let conflict = included.iter().any(|inc| {
            inc.body
                .inputs
                .iter()
                .any(|a| tx.body.inputs.iter().any(|b| a.prevout == b.prevout))
        });
        if conflict {
            continue;
        }
        size_budget -= sz;
        fees += fee;
        included.push(tx.clone());
    }

    let mut coinbase_data = height.to_le_bytes().to_vec();
    coinbase_data.extend_from_slice(b"summerpoem");
    let coinbase = Transaction {
        body: TxBody {
            version: 1,
            inputs: vec![],
            outputs: vec![TxOutput {
                amount: block_reward(height) + fees,
                lock: Lock::P2pkh { pkh: payout_pkh },
            }],
            locktime: 0,
            coinbase_data,
        },
        witnesses: vec![],
    };

    let mut transactions = vec![coinbase];
    transactions.extend(included);

    let bits = state
        .expected_bits(&state.tip_hash())
        .expect("tip always known");
    let mut block = Block {
        header: BlockHeader {
            version: 1,
            prev: state.tip_hash(),
            tx_root: sump_core::Hash256::ZERO,
            witness_root: sump_core::Hash256::ZERO,
            time,
            bits,
            nonce: 0,
        },
        transactions,
    };
    block.header.tx_root = block.compute_tx_root();
    block.header.witness_root = block.compute_witness_root();
    block
}

/// Mine the template in place. Returns false if `max_iters` nonces were
/// exhausted without a solution.
pub fn mine_block(ctx: &PowContext, block: &mut Block, max_iters: u64) -> bool {
    let target = bits_to_target(block.header.bits).expect("valid bits");
    let msg = block.header.pow_message();
    match sump_pow::mine(ctx, &msg, target, block.header.nonce, max_iters) {
        Some(nonce) => {
            block.header.nonce = nonce;
            true
        }
        None => false,
    }
}

/// Convenience: template + mine + submit, for tests and regtest tooling.
pub fn mine_and_connect(
    state: &mut ChainState,
    ctx: &PowContext,
    txs: &[Transaction],
    payout_pkh: [u8; 20],
    now: u64,
) -> Result<Block, ValidationError> {
    let mut block = build_block_template(state, txs, payout_pkh, now);
    assert!(mine_block(ctx, &mut block, u64::MAX), "unbounded mine");
    state.add_block(block.clone())?;
    Ok(block)
}
