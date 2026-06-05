//! Stable cluster and node identifiers.
//!
//! Identity is generated once, at `auradb init` / cluster bootstrap, and then
//! persisted. A [`NodeId`] names one server process within a cluster; a
//! [`ClusterId`] names the cluster the node belongs to. Both are random and
//! collision-resistant so that two independently bootstrapped nodes never share
//! an identity by accident.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::ClusterError;

/// A stable, non-zero 64-bit node identifier.
///
/// Node id `0` is reserved to mean "no node" (for example, an unknown leader),
/// so a generated id is always non-zero. Rendered as 16 lowercase hex digits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(#[serde(with = "hex_u64")] u64);

impl NodeId {
    /// Construct a node id from a raw non-zero value.
    ///
    /// Returns `None` for `0`, which is reserved.
    pub const fn new(value: u64) -> Option<NodeId> {
        if value == 0 {
            None
        } else {
            Some(NodeId(value))
        }
    }

    /// Construct a node id from a raw value without checking, for tests and
    /// trusted internal callers. The value must be non-zero.
    pub const fn from_raw(value: u64) -> NodeId {
        NodeId(value)
    }

    /// Generate a fresh random node id from the operating system entropy source.
    pub fn generate() -> NodeId {
        loop {
            let v = rand::random::<u64>();
            if v != 0 {
                return NodeId(v);
            }
        }
    }

    /// The raw 64-bit value.
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

impl FromStr for NodeId {
    type Err = ClusterError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let v = u64::from_str_radix(s.trim(), 16)
            .map_err(|_| ClusterError::IdentityConflict(format!("invalid node id: {s}")))?;
        NodeId::new(v)
            .ok_or_else(|| ClusterError::IdentityConflict("node id must be non-zero".into()))
    }
}

/// A stable, non-zero 128-bit cluster identifier, rendered as 32 hex digits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClusterId(#[serde(with = "hex_u128")] u128);

impl ClusterId {
    /// Construct a cluster id from a raw non-zero value.
    pub const fn new(value: u128) -> Option<ClusterId> {
        if value == 0 {
            None
        } else {
            Some(ClusterId(value))
        }
    }

    /// Generate a fresh random cluster id.
    pub fn generate() -> ClusterId {
        loop {
            let v = rand::random::<u128>();
            if v != 0 {
                return ClusterId(v);
            }
        }
    }

    /// The raw 128-bit value.
    pub const fn get(self) -> u128 {
        self.0
    }
}

impl fmt::Display for ClusterId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:032x}", self.0)
    }
}

impl FromStr for ClusterId {
    type Err = ClusterError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let v = u128::from_str_radix(s.trim(), 16)
            .map_err(|_| ClusterError::IdentityConflict(format!("invalid cluster id: {s}")))?;
        ClusterId::new(v)
            .ok_or_else(|| ClusterError::IdentityConflict("cluster id must be non-zero".into()))
    }
}

mod hex_u64 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &u64, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format!("{v:016x}"))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
        let s = String::deserialize(d)?;
        u64::from_str_radix(s.trim(), 16).map_err(serde::de::Error::custom)
    }
}

mod hex_u128 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &u128, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format!("{v:032x}"))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u128, D::Error> {
        let s = String::deserialize(d)?;
        u128::from_str_radix(s.trim(), 16).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_id_zero_is_reserved() {
        assert!(NodeId::new(0).is_none());
        assert_eq!(NodeId::new(7).unwrap().get(), 7);
    }

    #[test]
    fn generated_node_ids_are_non_zero() {
        for _ in 0..1000 {
            assert_ne!(NodeId::generate().get(), 0);
        }
    }

    #[test]
    fn node_id_display_is_16_hex() {
        assert_eq!(NodeId::from_raw(0xab).to_string(), "00000000000000ab");
    }

    #[test]
    fn node_id_roundtrips_through_string() {
        let id = NodeId::from_raw(0xdead_beef);
        assert_eq!(id.to_string().parse::<NodeId>().unwrap(), id);
    }

    #[test]
    fn cluster_id_roundtrips_through_json() {
        let id = ClusterId::new(0x1234_5678_9abc).unwrap();
        let json = serde_json::to_string(&id).unwrap();
        let back: ClusterId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn invalid_ids_are_rejected() {
        assert!("zzz".parse::<NodeId>().is_err());
        assert!("0".parse::<NodeId>().is_err());
        assert!("0".parse::<ClusterId>().is_err());
    }
}
