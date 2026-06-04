//! Schema types: field types, field definitions, relationships, and collection
//! schemas, plus validation of records against a schema.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::value::{Document, Value};

/// The declared type of a field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum FieldType {
    /// A UUID / record-id string.
    Uuid,
    /// A UTF-8 string.
    String,
    /// A 64-bit integer.
    Int,
    /// A 64-bit float.
    Float,
    /// A boolean.
    Bool,
    /// A timestamp (epoch milliseconds).
    Timestamp,
    /// A nested JSON-like document.
    Document,
    /// Raw bytes.
    Bytes,
    /// A fixed-dimension float vector.
    Vector {
        /// The required dimensionality.
        dim: usize,
    },
}

impl FieldType {
    /// Validate that a value conforms to this field type.
    pub fn validate(&self, value: &Value) -> Result<()> {
        let ok = match (self, value) {
            (_, Value::Null) => true, // nullability handled by FieldDef
            (FieldType::Uuid, Value::Text(_)) => true,
            (FieldType::String, Value::Text(_)) => true,
            (FieldType::Int, Value::Int(_)) => true,
            (FieldType::Float, Value::Float(_)) => true,
            (FieldType::Float, Value::Int(_)) => true,
            (FieldType::Bool, Value::Bool(_)) => true,
            (FieldType::Timestamp, Value::Timestamp(_)) => true,
            (FieldType::Timestamp, Value::Int(_)) => true,
            (FieldType::Document, Value::Object(_)) => true,
            (FieldType::Bytes, Value::Bytes(_)) => true,
            (FieldType::Vector { dim }, Value::Vector(v)) => v.len() == *dim,
            _ => false,
        };
        if ok {
            Ok(())
        } else if let (FieldType::Vector { dim }, Value::Vector(v)) = (self, value) {
            Err(Error::SchemaViolation(format!(
                "vector dimension mismatch: expected {dim}, got {}",
                v.len()
            )))
        } else {
            Err(Error::SchemaViolation(format!(
                "expected {:?}, got value of type {}",
                self,
                value.type_name()
            )))
        }
    }
}

/// Behavior when a linked record is deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnDelete {
    /// Reject deletion while inbound links exist (referential integrity).
    #[default]
    Restrict,
    /// Allow deletion; dangling links become unresolved on read.
    SetNull,
}

/// The cardinality of a relationship.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cardinality {
    /// At most one linked record.
    ToOne,
    /// Zero or more linked records.
    ToMany,
}

/// A relationship (link) field on a collection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Relationship {
    /// The relationship field name (e.g. `workspace`).
    pub name: String,
    /// The target collection name.
    pub target: String,
    /// Single or multi cardinality.
    pub cardinality: Cardinality,
    /// Delete behavior for the target.
    #[serde(default)]
    pub on_delete: OnDelete,
}

/// A scalar / document / vector field definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldDef {
    /// The field name.
    pub name: String,
    /// The field's declared type.
    pub field_type: FieldType,
    /// Whether this field is the primary key.
    #[serde(default)]
    pub primary_key: bool,
    /// Whether values must be unique across the collection.
    #[serde(default)]
    pub unique: bool,
    /// Whether the field may be null / absent.
    #[serde(default = "default_true")]
    pub nullable: bool,
    /// Whether a secondary index should be maintained for this field.
    #[serde(default)]
    pub indexed: bool,
}

fn default_true() -> bool {
    true
}

impl FieldDef {
    /// Convenience constructor for a simple field.
    pub fn new(name: impl Into<String>, field_type: FieldType) -> Self {
        FieldDef {
            name: name.into(),
            field_type,
            primary_key: false,
            unique: false,
            nullable: true,
            indexed: false,
        }
    }
}

/// The kind of a named collection index (beyond the per-field `indexed` flag).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexKind {
    /// An equality index over a dotted document path (e.g. `profile.company`).
    DocumentPath,
    /// A tokenized full-text inverted index over a string field.
    FullText,
}

/// A named collection index over a field or document path.
///
/// Document-path indexes accelerate equality filters on nested document values;
/// full-text indexes support tokenized text search on a string field. These are
/// declared separately from [`FieldDef`] so a single field can carry several.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexDef {
    /// The dotted document path (document_path) or field name (full_text).
    pub path: String,
    /// The index kind.
    pub kind: IndexKind,
}

/// A complete schema for one collection (entity type).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CollectionSchema {
    /// The collection name.
    pub name: String,
    /// Scalar / document / vector field definitions.
    pub fields: Vec<FieldDef>,
    /// Relationship (link) fields.
    #[serde(default)]
    pub relationships: Vec<Relationship>,
    /// Named document-path and full-text indexes.
    #[serde(default)]
    pub indexes: Vec<IndexDef>,
}

impl CollectionSchema {
    /// Create an empty schema with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        CollectionSchema {
            name: name.into(),
            fields: Vec::new(),
            relationships: Vec::new(),
            indexes: Vec::new(),
        }
    }

    /// Builder: add a field.
    pub fn with_field(mut self, field: FieldDef) -> Self {
        self.fields.push(field);
        self
    }

    /// Builder: add a relationship.
    pub fn with_relationship(mut self, rel: Relationship) -> Self {
        self.relationships.push(rel);
        self
    }

    /// Builder: add a named index.
    pub fn with_index(mut self, index: IndexDef) -> Self {
        self.indexes.push(index);
        self
    }

    /// The dotted paths with a document-path index.
    pub fn document_path_indexes(&self) -> impl Iterator<Item = &str> {
        self.indexes
            .iter()
            .filter(|i| i.kind == IndexKind::DocumentPath)
            .map(|i| i.path.as_str())
    }

    /// The field names with a full-text index.
    pub fn full_text_indexes(&self) -> impl Iterator<Item = &str> {
        self.indexes
            .iter()
            .filter(|i| i.kind == IndexKind::FullText)
            .map(|i| i.path.as_str())
    }

    /// The primary-key field, if one is declared.
    pub fn primary_key(&self) -> Option<&FieldDef> {
        self.fields.iter().find(|f| f.primary_key)
    }

    /// Look up a field by name.
    pub fn field(&self, name: &str) -> Option<&FieldDef> {
        self.fields.iter().find(|f| f.name == name)
    }

    /// Look up a relationship by name.
    pub fn relationship(&self, name: &str) -> Option<&Relationship> {
        self.relationships.iter().find(|r| r.name == name)
    }

    /// Validate the schema definition itself for internal consistency.
    pub fn validate_definition(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            return Err(Error::SchemaViolation("collection name is empty".into()));
        }
        let pk_count = self.fields.iter().filter(|f| f.primary_key).count();
        if pk_count > 1 {
            return Err(Error::SchemaViolation(format!(
                "collection {} declares {pk_count} primary keys",
                self.name
            )));
        }
        for rel in &self.relationships {
            if self.field(&rel.name).is_some() {
                return Err(Error::SchemaViolation(format!(
                    "relationship {} collides with a field of the same name",
                    rel.name
                )));
            }
        }
        for index in &self.indexes {
            self.validate_index(index)?;
        }
        Ok(())
    }

    fn validate_index(&self, index: &IndexDef) -> Result<()> {
        if index.path.trim().is_empty() {
            return Err(Error::SchemaViolation("index path is empty".into()));
        }
        let segments: Vec<&str> = index.path.split('.').collect();
        if segments.iter().any(|s| s.is_empty()) {
            return Err(Error::SchemaViolation(format!(
                "index path {} has an empty segment",
                index.path
            )));
        }
        let root = segments[0];
        let field = self.field(root).ok_or_else(|| {
            Error::SchemaViolation(format!(
                "index path {} references unknown field {root}",
                index.path
            ))
        })?;
        match index.kind {
            IndexKind::DocumentPath => {
                if segments.len() > 1 && field.field_type != FieldType::Document {
                    return Err(Error::SchemaViolation(format!(
                        "document-path index {} requires {root} to be a document field",
                        index.path
                    )));
                }
            }
            IndexKind::FullText => {
                if segments.len() != 1 {
                    return Err(Error::SchemaViolation(format!(
                        "full-text index {} must target a single string field",
                        index.path
                    )));
                }
                if field.field_type != FieldType::String {
                    return Err(Error::SchemaViolation(format!(
                        "full-text index {} requires {root} to be a string field",
                        index.path
                    )));
                }
            }
        }
        Ok(())
    }

    /// Validate a record's fields against this schema. Checks declared field
    /// types, required (non-nullable) fields, primary-key presence, and that
    /// relationship fields, when present, are record-id strings.
    pub fn validate_record(&self, fields: &Document) -> Result<()> {
        for def in &self.fields {
            match fields.get(&def.name) {
                None | Some(Value::Null) => {
                    if !def.nullable {
                        return Err(Error::SchemaViolation(format!(
                            "field {} is required",
                            def.name
                        )));
                    }
                    if def.primary_key {
                        return Err(Error::SchemaViolation(format!(
                            "primary key {} must be present",
                            def.name
                        )));
                    }
                }
                Some(value) => def.field_type.validate(value)?,
            }
        }
        for rel in &self.relationships {
            if let Some(value) = fields.get(&rel.name) {
                match (rel.cardinality, value) {
                    (_, Value::Null) => {}
                    (Cardinality::ToOne, Value::Text(_)) => {}
                    (Cardinality::ToMany, Value::Array(items)) => {
                        for item in items {
                            if !matches!(item, Value::Text(_)) {
                                return Err(Error::SchemaViolation(format!(
                                    "relationship {} targets must be record-id strings",
                                    rel.name
                                )));
                            }
                        }
                    }
                    _ => {
                        return Err(Error::SchemaViolation(format!(
                            "relationship {} has wrong shape for its cardinality",
                            rel.name
                        )));
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_schema() -> CollectionSchema {
        CollectionSchema::new("Document")
            .with_field(FieldDef {
                name: "id".into(),
                field_type: FieldType::Uuid,
                primary_key: true,
                unique: true,
                nullable: false,
                indexed: false,
            })
            .with_field(FieldDef {
                name: "title".into(),
                field_type: FieldType::String,
                primary_key: false,
                unique: false,
                nullable: false,
                indexed: true,
            })
            .with_field(FieldDef::new("embedding", FieldType::Vector { dim: 3 }))
            .with_relationship(Relationship {
                name: "owner".into(),
                target: "User".into(),
                cardinality: Cardinality::ToOne,
                on_delete: OnDelete::Restrict,
            })
    }

    #[test]
    fn valid_record_passes() {
        let schema = doc_schema();
        let mut fields = Document::new();
        fields.insert("id".into(), Value::Text("abc".into()));
        fields.insert("title".into(), Value::Text("Hello".into()));
        fields.insert("embedding".into(), Value::Vector(vec![1.0, 2.0, 3.0]));
        fields.insert("owner".into(), Value::Text("user-1".into()));
        schema.validate_record(&fields).unwrap();
    }

    #[test]
    fn missing_required_field_fails() {
        let schema = doc_schema();
        let mut fields = Document::new();
        fields.insert("id".into(), Value::Text("abc".into()));
        let err = schema.validate_record(&fields).unwrap_err();
        assert!(matches!(err, Error::SchemaViolation(_)));
    }

    #[test]
    fn vector_dimension_enforced() {
        let schema = doc_schema();
        let mut fields = Document::new();
        fields.insert("id".into(), Value::Text("abc".into()));
        fields.insert("title".into(), Value::Text("Hello".into()));
        fields.insert("embedding".into(), Value::Vector(vec![1.0, 2.0]));
        let err = schema.validate_record(&fields).unwrap_err();
        assert!(err.to_string().contains("dimension"));
    }

    #[test]
    fn wrong_type_fails() {
        let schema = doc_schema();
        let mut fields = Document::new();
        fields.insert("id".into(), Value::Text("abc".into()));
        fields.insert("title".into(), Value::Int(5));
        assert!(schema.validate_record(&fields).is_err());
    }

    #[test]
    fn schema_roundtrips_json() {
        let schema = doc_schema();
        let json = serde_json::to_string(&schema).unwrap();
        let back: CollectionSchema = serde_json::from_str(&json).unwrap();
        assert_eq!(schema, back);
    }

    #[test]
    fn duplicate_primary_keys_rejected() {
        let schema = CollectionSchema::new("X")
            .with_field(FieldDef {
                name: "a".into(),
                field_type: FieldType::Int,
                primary_key: true,
                unique: true,
                nullable: false,
                indexed: false,
            })
            .with_field(FieldDef {
                name: "b".into(),
                field_type: FieldType::Int,
                primary_key: true,
                unique: true,
                nullable: false,
                indexed: false,
            });
        assert!(schema.validate_definition().is_err());
    }
}
