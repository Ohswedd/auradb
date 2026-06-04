//! The record type: a stored entity instance.

use serde::{Deserialize, Serialize};

use crate::ids::{CollectionId, RecordId, TxnId};
use crate::value::{Document, Value};

/// A stored record: a logical identity plus its field document and version
/// metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    /// Stable logical identity.
    pub id: RecordId,
    /// The collection this record belongs to.
    pub collection: CollectionId,
    /// The record's fields.
    pub fields: Document,
    /// Monotonic per-record version, incremented on each update.
    #[serde(default)]
    pub version: u64,
    /// The transaction that produced this version.
    #[serde(default = "auto_txn")]
    pub created_txn: TxnId,
}

fn auto_txn() -> TxnId {
    TxnId::AUTO
}

impl Record {
    /// Construct a new record at version 1.
    pub fn new(id: RecordId, collection: CollectionId, fields: Document) -> Self {
        Record {
            id,
            collection,
            fields,
            version: 1,
            created_txn: TxnId::AUTO,
        }
    }

    /// Read a field by name.
    pub fn get(&self, field: &str) -> Option<&Value> {
        self.fields.get(field)
    }

    /// Resolve a dotted document path (e.g. `metadata.status`).
    pub fn get_path(&self, path: &str) -> Option<&Value> {
        let (head, rest) = match path.split_once('.') {
            Some((h, r)) => (h, Some(r)),
            None => (path, None),
        };
        let value = self.fields.get(head)?;
        match rest {
            Some(rest) => value.get_path(rest),
            None => Some(value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_roundtrips_json() {
        let mut fields = Document::new();
        fields.insert("title".into(), Value::Text("Hi".into()));
        let rec = Record::new(RecordId::from_u128(7), CollectionId::new("Doc"), fields);
        let json = serde_json::to_string(&rec).unwrap();
        let back: Record = serde_json::from_str(&json).unwrap();
        assert_eq!(rec, back);
    }

    #[test]
    fn nested_path_lookup() {
        let mut meta = Document::new();
        meta.insert("status".into(), Value::Text("published".into()));
        let mut fields = Document::new();
        fields.insert("metadata".into(), Value::Object(meta));
        let rec = Record::new(RecordId::from_u128(1), CollectionId::new("Doc"), fields);
        assert_eq!(
            rec.get_path("metadata.status"),
            Some(&Value::Text("published".into()))
        );
    }
}
