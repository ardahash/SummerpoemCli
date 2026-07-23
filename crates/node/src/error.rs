use sump_core::encode::DecodeError;
use sump_core::tx::OutPoint;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("unknown previous block")]
    UnknownParent,
    #[error("block already known")]
    Duplicate,
    #[error("wrong difficulty bits (expected {expected:#010x}, got {got:#010x})")]
    WrongBits { expected: u32, got: u32 },
    #[error("invalid difficulty bits encoding")]
    BadBits,
    #[error("proof of work does not meet target")]
    BadPow,
    #[error("block timestamp not after median-time-past")]
    TimeTooOld,
    #[error("block exceeds maximum size")]
    TooLarge,
    #[error("merkle root mismatch")]
    BadMerkleRoot,
    #[error("witness root mismatch")]
    BadWitnessRoot,
    #[error("first transaction must be the coinbase")]
    MissingCoinbase,
    #[error("unexpected extra coinbase")]
    ExtraCoinbase,
    #[error("bad coinbase data")]
    BadCoinbaseData,
    #[error("coinbase pays more than reward plus fees")]
    CoinbaseOverpay,
    #[error("transaction has no inputs")]
    NoInputs,
    #[error("transaction has no outputs")]
    NoOutputs,
    #[error("duplicate transaction in block")]
    DuplicateTx,
    #[error("unknown or spent input {0:?}")]
    UnknownInput(OutPoint),
    #[error("coinbase output spent before maturity")]
    ImmatureCoinbase,
    #[error("output is timelocked until height {0}")]
    Timelocked(u64),
    #[error("witness count does not match input count")]
    WitnessMismatch,
    #[error("public key does not match output")]
    WrongPubkey,
    #[error("invalid signature")]
    BadSignature,
    #[error("input value below output value")]
    InsufficientInputs,
    #[error("value overflow")]
    Overflow,
    #[error("zero-amount output")]
    ZeroOutput,
    #[error("invalid genesis block")]
    BadGenesis,
    #[error("decode error: {0}")]
    Decode(#[from] DecodeError),
}
