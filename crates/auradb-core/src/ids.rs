//! Stable identifier types.
//!
//! Identity in AuraDB is always logical, never a physical offset. Records are
//! addressed by [`RecordId`]; transactions by [`TxnId`]; collections by name via
//! [`CollectionId`].

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::Error;

/// A stable, immutable 128-bit record identifier.
///
/// Record ids are logical identities that survive compaction and recovery. They
/// are rendered as 32 lowercase hex characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RecordId(#[serde(with = "hex_u128")] pub u128);

impl RecordId {
    /// Construct a record id from a raw 128-bit value.
    pub const fn from_u128(v: u128) -> Self {
        RecordId(v)
    }

    /// The raw 128-bit value.
    pub const fn as_u128(self) -> u128 {
        self.0
    }

    /// The 16-byte big-endian representation.
    pub const fn to_bytes(self) -> [u8; 16] {
        self.0.to_be_bytes()
    }

    /// Construct a record id from 16 big-endian bytes.
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        RecordId(u128::from_be_bytes(bytes))
    }
}

impl fmt::Display for RecordId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:032x}", self.0)
    }
}

impl FromStr for RecordId {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Accept both plain hex and dashed UUID form.
        let cleaned: String = s.chars().filter(|c| *c != '-').collect();
        let v = u128::from_str_radix(&cleaned, 16)
            .map_err(|_| Error::InvalidRequest(format!("invalid record id: {s}")))?;
        Ok(RecordId(v))
    }
}

mod hex_u128 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &u128, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format!("{v:032x}"))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u128, D::Error> {
        let s = String::deserialize(d)?;
        let cleaned: String = s.chars().filter(|c| *c != '-').collect();
        u128::from_str_radix(&cleaned, 16).map_err(serde::de::Error::custom)
    }
}

/// A monotonically increasing transaction identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TxnId(pub u64);

impl TxnId {
    /// The transaction id used for non-transactional (auto-commit) writes.
    pub const AUTO: TxnId = TxnId(0);

    /// The raw value.
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for TxnId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "txn-{}", self.0)
    }
}

/// A logical collection (entity type) identifier: its name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CollectionId(pub String);

impl CollectionId {
    /// Construct from anything string-like.
    pub fn new(name: impl Into<String>) -> Self {
        CollectionId(name.into())
    }

    /// The collection name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CollectionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for CollectionId {
    fn from(s: &str) -> Self {
        CollectionId(s.to_string())
    }
}

/// A schema version identifier, incremented on each schema change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SchemaId(pub u64);

impl fmt::Display for SchemaId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "schema-v{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_id_display_is_32_hex() {
        let id = RecordId::from_u128(0x1234);
        assert_eq!(id.to_string(), "00000000000000000000000000001234");
    }

    #[test]
    fn record_id_roundtrips_bytes() {
        let id = RecordId::from_u128(0xdead_beef_0000_1111_2222_3333_4444_5555);
        assert_eq!(RecordId::from_bytes(id.to_bytes()), id);
    }

    #[test]
    fn record_id_parses_hex_and_uuid() {
        let a: RecordId = "00000000000000000000000000001234".parse().unwrap();
        assert_eq!(a, RecordId::from_u128(0x1234));
        let b: RecordId = "550e8400-e29b-41d4-a716-446655440000".parse().unwrap();
        assert_eq!(b.to_string(), "550e8400e29b41d4a716446655440000");
    }

    #[test]
    fn record_id_json_is_string() {
        let id = RecordId::from_u128(255);
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"000000000000000000000000000000ff\"");
        let back: RecordId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
    }

    #[test]
    fn invalid_record_id_errors() {
        assert!("not-hex-zzz".parse::<RecordId>().is_err());
    }
}
