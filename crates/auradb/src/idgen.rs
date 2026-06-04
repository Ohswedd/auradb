//! Deterministic record-id derivation.
//!
//! A record's logical [`RecordId`] is derived deterministically from its
//! collection name and primary-key value using a 128-bit FNV-1a hash. This makes
//! identity stable across restarts and makes upsert-by-primary-key well defined,
//! without requiring the client to supply a 128-bit id.

use auradb_core::{CollectionSchema, Document, Error, RecordId, Result, Value};

const FNV_OFFSET: u128 = 0x6c62272e07bb014262b821756295c58d;
const FNV_PRIME: u128 = 0x0000000001000000000000000000013b;

/// 128-bit FNV-1a hash of a byte slice.
pub fn fnv1a_128(bytes: &[u8]) -> u128 {
    let mut hash = FNV_OFFSET;
    for &b in bytes {
        hash ^= b as u128;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Canonical string form of a value for hashing.
fn canonical(value: &Value) -> String {
    serde_json::to_string(&value.to_json()).unwrap_or_default()
}

/// Derive the [`RecordId`] for a primary-key value in a collection. This is the
/// inverse used to resolve relationship links, which store the target's
/// primary-key value rather than its internal id.
pub fn derive_id(collection: &str, pk_value: &Value) -> RecordId {
    let seed = format!("{}\u{0}{}", collection, canonical(pk_value));
    RecordId::from_u128(fnv1a_128(seed.as_bytes()))
}

/// Derive the record id for a record being written to `schema`.
///
/// If the schema has a primary key, the id is derived from the collection name
/// and the primary-key value (so the same key always maps to the same id). If
/// there is no primary key, `fallback` (a monotonic counter value) is used.
pub fn record_id_for(
    schema: &CollectionSchema,
    fields: &Document,
    fallback: u64,
) -> Result<RecordId> {
    match schema.primary_key() {
        Some(pk) => {
            let value = fields.get(&pk.name).ok_or_else(|| {
                Error::SchemaViolation(format!("primary key {} is required", pk.name))
            })?;
            if value.is_null() {
                return Err(Error::SchemaViolation(format!(
                    "primary key {} must not be null",
                    pk.name
                )));
            }
            Ok(derive_id(&schema.name, value))
        }
        None => Ok(RecordId::from_u128(fallback as u128)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auradb_core::{FieldDef, FieldType};

    fn schema() -> CollectionSchema {
        CollectionSchema::new("User").with_field(FieldDef {
            name: "id".into(),
            field_type: FieldType::Uuid,
            primary_key: true,
            unique: true,
            nullable: false,
            indexed: false,
        })
    }

    #[test]
    fn same_pk_same_id() {
        let mut f = Document::new();
        f.insert("id".into(), Value::Text("abc".into()));
        let a = record_id_for(&schema(), &f, 0).unwrap();
        let b = record_id_for(&schema(), &f, 0).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn different_pk_different_id() {
        let mut f1 = Document::new();
        f1.insert("id".into(), Value::Text("abc".into()));
        let mut f2 = Document::new();
        f2.insert("id".into(), Value::Text("xyz".into()));
        assert_ne!(
            record_id_for(&schema(), &f1, 0).unwrap(),
            record_id_for(&schema(), &f2, 0).unwrap()
        );
    }

    #[test]
    fn missing_pk_errors() {
        assert!(record_id_for(&schema(), &Document::new(), 0).is_err());
    }

    #[test]
    fn no_pk_uses_fallback() {
        let s = CollectionSchema::new("Log").with_field(FieldDef::new("msg", FieldType::String));
        let id = record_id_for(&s, &Document::new(), 99).unwrap();
        assert_eq!(id, RecordId::from_u128(99));
    }
}
