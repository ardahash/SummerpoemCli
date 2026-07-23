//! SHA3-256 binary merkle tree (Bitcoin-style odd-leaf duplication).

use crate::hash::{sha3, Hash256};

pub fn merkle_root(leaves: Vec<Hash256>) -> Hash256 {
    if leaves.is_empty() {
        return sha3(&[b"sump/merkle/empty"]);
    }
    // domain-separate leaves from internal nodes to prevent second-preimage
    // ambiguity between the two levels
    let mut layer: Vec<Hash256> = leaves
        .iter()
        .map(|l| sha3(&[b"sump/merkle/leaf", &l.0]))
        .collect();
    while layer.len() > 1 {
        if layer.len() % 2 == 1 {
            layer.push(*layer.last().unwrap());
        }
        layer = layer
            .chunks(2)
            .map(|p| sha3(&[b"sump/merkle/node", &p[0].0, &p[1].0]))
            .collect();
    }
    layer[0]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_order_sensitive() {
        let a = sha3(&[b"a"]);
        let b = sha3(&[b"b"]);
        assert_eq!(merkle_root(vec![a, b]), merkle_root(vec![a, b]));
        assert_ne!(merkle_root(vec![a, b]), merkle_root(vec![b, a]));
        assert_ne!(merkle_root(vec![a]), merkle_root(vec![a, a]));
        // single leaf root is not the leaf itself (domain separated)
        assert_ne!(merkle_root(vec![a]), a);
    }
}
