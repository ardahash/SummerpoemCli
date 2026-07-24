//! Summerpoem P2P networking: ML-KEM-768 encrypted transport, block/tx
//! gossip, and chain synchronization.

pub mod message;
pub mod node;
pub mod transport;

pub use message::Message;
pub use node::NetNode;
