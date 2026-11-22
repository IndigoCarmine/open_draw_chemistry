//! Names of nodes and of custom nodes.

use std::hash::{BuildHasher, Hasher};

use serde::{Deserialize, Serialize};

/// A node, unique within its graph: ids are never handed out twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(pub u64);

/// A custom node's identity: 32 hex digits, random, the same in every graph and library that
/// holds the node.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DefId(pub String);

impl DefId {
    /// A new id no other custom node has (128 random bits).
    pub fn fresh() -> DefId {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let mut words = [0u64; 2];
        for (k, w) in words.iter_mut().enumerate() {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u64(
                COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed) ^ (k as u64) << 63,
            );
            if let Ok(t) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
                h.write_u128(t.as_nanos());
            }
            h.write_usize(std::process::id() as usize);
            *w = h.finish();
        }
        DefId(format!("{:016x}{:016x}", words[0], words[1]))
    }

    /// The first eight digits, for file names.
    pub fn short(&self) -> &str {
        self.0.get(..8).unwrap_or(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_ids_differ() {
        let a = DefId::fresh();
        let b = DefId::fresh();
        assert_ne!(a, b);
        assert_eq!(a.0.len(), 32);
        assert_eq!(a.short().len(), 8);
    }
}
