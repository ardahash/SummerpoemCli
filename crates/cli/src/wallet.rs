//! CLI adapter over the shared `sump-wallet` library: pulls the wallet's own
//! UTXOs out of a local `ChainState` and delegates balance/send to the
//! library so key derivation stays identical to the standalone wallet.

use anyhow::Result;
use sump_core::params::Params;
use sump_core::tx::{SigScheme, Transaction};
use sump_node::ChainState;
use sump_wallet::{balances, build_send, OwnedUtxo};

pub use sump_wallet::Wallet;

/// Collect the wallet's own unspent outputs from the local chain state.
fn owned_utxos(wallet: &Wallet, state: &ChainState) -> Vec<OwnedUtxo> {
    let ids = wallet.owned_ids();
    state
        .utxos()
        .iter()
        .filter(|(_, u)| ids.contains(&(u.output.lock.scheme().id(), *u.output.lock.pkh())))
        .map(|(op, u)| OwnedUtxo {
            outpoint: *op,
            output: u.output.clone(),
            coinbase: u.coinbase,
            height: u.height,
        })
        .collect()
}

/// Spendable balance in stanzas.
pub fn balance(wallet: &Wallet, state: &ChainState) -> u64 {
    let utxos = owned_utxos(wallet, state);
    balances(&utxos, state.height() + 1, state.params().coinbase_maturity).0
}

/// Build and sign a payment against the local chain state.
pub fn send(
    wallet: &Wallet,
    state: &ChainState,
    to_scheme: SigScheme,
    to_pkh: [u8; 20],
    amount: u64,
    fee: u64,
) -> Result<Transaction> {
    let utxos = owned_utxos(wallet, state);
    let tx = build_send(
        wallet,
        &utxos,
        state.height() + 1,
        state.params().coinbase_maturity,
        to_scheme,
        to_pkh,
        amount,
        fee,
    )?;
    Ok(tx)
}

pub fn address(wallet: &Wallet, params: &Params, index: u32) -> String {
    wallet.address(params.address_hrp, index)
}

pub fn vault_address(wallet: &Wallet, params: &Params, index: u32) -> String {
    wallet.vault_address(params.address_hrp, index)
}
