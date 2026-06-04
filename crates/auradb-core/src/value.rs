//! The AuraDB value model.
//!
//! [`Value`] is the universal data type stored in records and carried in the
//! Query IR. It is a superset of JSON with explicit support for vectors,
//! timestamps, and binary blobs. Plain JSON scalars, arrays, and objects map
//! directly so the wire format stays compatible with Aura Connector's JSON
//! payloads; the extension types use reserved `$`-prefixed object keys.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A document is an ordered map of field name to [`Value`].
pub type Document = BTreeMap<String, Value>;

/// A single AuraDB value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// The null value.
    Null,
    /// A boolean.
    Bool(bool),
    /// A 64-bit signed integer.
    Int(i64),
    /// A 64-bit float.
    Float(f64),
    /// A UTF-8 string.
    Text(String),
    /// Raw bytes, encoded on the wire as `{"$bytes": [..]}`.
    Bytes(Vec<u8>),
    /// A timestamp in epoch milliseconds, encoded as `{"$timestamp": ms}`.
    Timestamp(i64),
    /// A dense float vector, encoded as `{"$vector": [..]}`.
    Vector(Vec<f32>),
    /// An ordered array of values.
    Array(Vec<Value>),
    /// A nested document / object.
    Object(Document),
}

impl Value {
    /// A human-readable name for this value's type (used in error messages).
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::Text(_) => "string",
            Value::Bytes(_) => "bytes",
            Value::Timestamp(_) => "timestamp",
            Value::Vector(_) => "vector",
            Value::Array(_) => "array",
            Value::Object(_) => "document",
        }
    }

    /// Whether this value is null.
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// Borrow as a string slice if this is text.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Value::Text(s) => Some(s),
            _ => None,
        }
    }

    /// Borrow as a vector if this is a vector value.
    pub fn as_vector(&self) -> Option<&[f32]> {
        match self {
            Value::Vector(v) => Some(v),
            _ => None,
        }
    }

    /// Borrow as an object if this is a document.
    pub fn as_object(&self) -> Option<&Document> {
        match self {
            Value::Object(m) => Some(m),
            _ => None,
        }
    }

    /// Interpret as `f64` if numeric (int or float).
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }

    /// Resolve a dotted document path such as `metadata.status` against this
    /// value, returning the value at that path if present.
    pub fn get_path(&self, path: &str) -> Option<&Value> {
        let mut current = self;
        for segment in path.split('.') {
            match current {
                Value::Object(map) => current = map.get(segment)?,
                _ => return None,
            }
        }
        Some(current)
    }

    /// Convert from a `serde_json::Value`, recognizing the `$`-prefixed
    /// extension encodings for vectors, timestamps, and bytes.
    pub fn from_json(json: serde_json::Value) -> Value {
        use serde_json::Value as J;
        match json {
            J::Null => Value::Null,
            J::Bool(b) => Value::Bool(b),
            J::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Value::Int(i)
                } else {
                    Value::Float(n.as_f64().unwrap_or(f64::NAN))
                }
            }
            J::String(s) => Value::Text(s),
            J::Array(items) => Value::Array(items.into_iter().map(Value::from_json).collect()),
            J::Object(map) => {
                if map.len() == 1 {
                    if let Some(J::Array(items)) = map.get("$vector") {
                        if let Some(v) = json_array_to_vector(items) {
                            return Value::Vector(v);
                        }
                    }
                    if let Some(J::Number(n)) = map.get("$timestamp") {
                        if let Some(ms) = n.as_i64() {
                            return Value::Timestamp(ms);
                        }
                    }
                    if let Some(J::Array(items)) = map.get("$bytes") {
                        if let Some(bytes) = json_array_to_bytes(items) {
                            return Value::Bytes(bytes);
                        }
                    }
                }
                Value::Object(
                    map.into_iter()
                        .map(|(k, v)| (k, Value::from_json(v)))
                        .collect(),
                )
            }
        }
    }

    /// Convert to a `serde_json::Value` using the extension encodings.
    pub fn to_json(&self) -> serde_json::Value {
        use serde_json::Value as J;
        match self {
            Value::Null => J::Null,
            Value::Bool(b) => J::Bool(*b),
            Value::Int(i) => J::Number((*i).into()),
            Value::Float(f) => serde_json::Number::from_f64(*f)
                .map(J::Number)
                .unwrap_or(J::Null),
            Value::Text(s) => J::String(s.clone()),
            Value::Bytes(b) => {
                let mut m = serde_json::Map::new();
                m.insert(
                    "$bytes".to_string(),
                    J::Array(b.iter().map(|x| J::Number((*x as u64).into())).collect()),
                );
                J::Object(m)
            }
            Value::Timestamp(ms) => {
                let mut m = serde_json::Map::new();
                m.insert("$timestamp".to_string(), J::Number((*ms).into()));
                J::Object(m)
            }
            Value::Vector(v) => {
                let mut m = serde_json::Map::new();
                m.insert(
                    "$vector".to_string(),
                    J::Array(
                        v.iter()
                            .map(|x| {
                                serde_json::Number::from_f64(*x as f64)
                                    .map(J::Number)
                                    .unwrap_or(J::Null)
                            })
                            .collect(),
                    ),
                );
                J::Object(m)
            }
            Value::Array(items) => J::Array(items.iter().map(Value::to_json).collect()),
            Value::Object(map) => {
                J::Object(map.iter().map(|(k, v)| (k.clone(), v.to_json())).collect())
            }
        }
    }
}

fn json_array_to_vector(items: &[serde_json::Value]) -> Option<Vec<f32>> {
    items.iter().map(|v| v.as_f64().map(|f| f as f32)).collect()
}

fn json_array_to_bytes(items: &[serde_json::Value]) -> Option<Vec<u8>> {
    items
        .iter()
        .map(|v| v.as_u64().and_then(|n| u8::try_from(n).ok()))
        .collect()
}

impl Serialize for Value {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_json().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Value {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let json = serde_json::Value::deserialize(deserializer)?;
        Ok(Value::from_json(json))
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::Text(s.to_string())
    }
}
impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::Text(s)
    }
}
impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Value::Int(v)
    }
}
impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Value::Float(v)
    }
}
impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::Bool(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_json_roundtrip() {
        for v in [
            Value::Null,
            Value::Bool(true),
            Value::Int(-7),
            Value::Float(1.5),
            Value::Text("hi".into()),
        ] {
            let json = serde_json::to_string(&v).unwrap();
            let back: Value = serde_json::from_str(&json).unwrap();
            assert_eq!(v, back);
        }
    }

    #[test]
    fn plain_scalars_use_natural_json() {
        assert_eq!(serde_json::to_string(&Value::Int(42)).unwrap(), "42");
        assert_eq!(
            serde_json::to_string(&Value::Text("x".into())).unwrap(),
            "\"x\""
        );
        assert_eq!(serde_json::to_string(&Value::Bool(false)).unwrap(), "false");
    }

    #[test]
    fn vector_roundtrip() {
        let v = Value::Vector(vec![0.1, 0.2, 0.3]);
        let json = serde_json::to_string(&v).unwrap();
        assert!(json.contains("$vector"));
        let back: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn timestamp_and_bytes_roundtrip() {
        for v in [
            Value::Timestamp(1717459200000),
            Value::Bytes(vec![1, 2, 255]),
        ] {
            let json = serde_json::to_string(&v).unwrap();
            let back: Value = serde_json::from_str(&json).unwrap();
            assert_eq!(v, back);
        }
    }

    #[test]
    fn nested_document_roundtrip() {
        let mut inner = Document::new();
        inner.insert("status".into(), Value::Text("published".into()));
        inner.insert(
            "tags".into(),
            Value::Array(vec![Value::Int(1), Value::Int(2)]),
        );
        let v = Value::Object({
            let mut m = Document::new();
            m.insert("metadata".into(), Value::Object(inner));
            m
        });
        let json = serde_json::to_string(&v).unwrap();
        let back: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn document_path_access() {
        let mut inner = Document::new();
        inner.insert("status".into(), Value::Text("published".into()));
        let mut outer = Document::new();
        outer.insert("metadata".into(), Value::Object(inner));
        let v = Value::Object(outer);
        assert_eq!(
            v.get_path("metadata.status"),
            Some(&Value::Text("published".into()))
        );
        assert_eq!(v.get_path("metadata.missing"), None);
        assert_eq!(v.get_path("nope"), None);
    }
}
