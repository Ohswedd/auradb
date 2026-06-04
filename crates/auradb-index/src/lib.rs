//! # auradb-index
//!
//! In-memory indexes for one collection, rebuilt from storage on open and kept
//! consistent on every mutation:
//!
//! - a **primary key** map (unique),
//! - **unique** field maps (enforce uniqueness on insert/update),
//! - **secondary** field maps (equality lookup acceleration),
//! - **exact vector** indexes behind the [`VectorIndex`] trait.
//!
//! Indexes are equality-oriented; ordering, ranges, and `contains` are handled
//! by the query engine over candidate sets. Vector search is exact (full scan);
//! the [`VectorIndex`] trait leaves room for an ANN implementation later without
//! changing the query engine.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod metric;

use std::collections::HashMap;

use auradb_core::{CollectionSchema, Error, FieldType, Record, RecordId, Result, Value};

pub use metric::{metric_json, Metric};

/// A canonical, hashable key derived from a [`Value`] for equality indexing.
fn index_key(value: &Value) -> String {
    serde_json::to_string(&value.to_json()).unwrap_or_default()
}

/// A vector search result.
#[derive(Debug, Clone, PartialEq)]
pub struct Neighbor {
    /// The matching record id.
    pub id: RecordId,
    /// Similarity score (higher is more similar).
    pub score: f32,
    /// Human-facing distance (lower is more similar).
    pub distance: f32,
}

/// A vector index over the vectors of one field.
///
/// The default implementation is exact (full scan). Implementors may provide
/// approximate indexes without changing the query engine.
pub trait VectorIndex: Send {
    /// Insert or replace the vector for `id`.
    fn insert(&mut self, id: RecordId, vector: Vec<f32>);
    /// Remove the vector for `id`, if present.
    fn remove(&mut self, id: RecordId);
    /// Return up to `k` nearest neighbours to `query` under `metric`.
    fn nearest(&self, query: &[f32], k: usize, metric: Metric) -> Vec<Neighbor>;
    /// The number of indexed vectors.
    fn len(&self) -> usize;
    /// Whether the index is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// An exact (brute-force) vector index.
#[derive(Debug, Default)]
pub struct ExactVectorIndex {
    dim: usize,
    entries: HashMap<RecordId, Vec<f32>>,
}

impl ExactVectorIndex {
    /// Create an empty exact index for `dim`-dimensional vectors.
    pub fn new(dim: usize) -> Self {
        ExactVectorIndex {
            dim,
            entries: HashMap::new(),
        }
    }

    /// The expected dimensionality.
    pub fn dim(&self) -> usize {
        self.dim
    }
}

impl VectorIndex for ExactVectorIndex {
    fn insert(&mut self, id: RecordId, vector: Vec<f32>) {
        self.entries.insert(id, vector);
    }

    fn remove(&mut self, id: RecordId) {
        self.entries.remove(&id);
    }

    fn nearest(&self, query: &[f32], k: usize, metric: Metric) -> Vec<Neighbor> {
        let mut scored: Vec<Neighbor> = self
            .entries
            .iter()
            .filter(|(_, v)| v.len() == query.len())
            .map(|(id, v)| Neighbor {
                id: *id,
                score: metric.similarity(query, v),
                distance: metric.distance(query, v),
            })
            .collect();
        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.id.cmp(&b.id))
        });
        scored.truncate(k);
        scored
    }

    fn len(&self) -> usize {
        self.entries.len()
    }
}

/// The full set of indexes for one collection.
pub struct CollectionIndexes {
    primary_field: Option<String>,
    unique_fields: Vec<String>,
    secondary_fields: Vec<String>,
    /// field name -> value key -> record id (unique, incl. primary key).
    unique_maps: HashMap<String, HashMap<String, RecordId>>,
    /// field name -> value key -> record ids (non-unique).
    secondary_maps: HashMap<String, HashMap<String, Vec<RecordId>>>,
    /// field name -> exact vector index.
    vector_maps: HashMap<String, ExactVectorIndex>,
}

impl CollectionIndexes {
    /// Build empty indexes from a schema's field roles.
    pub fn from_schema(schema: &CollectionSchema) -> Self {
        let mut primary_field = None;
        let mut unique_fields = Vec::new();
        let mut secondary_fields = Vec::new();
        let mut unique_maps = HashMap::new();
        let mut secondary_maps = HashMap::new();
        let mut vector_maps = HashMap::new();

        for field in &schema.fields {
            if field.primary_key {
                primary_field = Some(field.name.clone());
            }
            if field.primary_key || field.unique {
                unique_fields.push(field.name.clone());
                unique_maps.insert(field.name.clone(), HashMap::new());
            } else if field.indexed {
                secondary_fields.push(field.name.clone());
                secondary_maps.insert(field.name.clone(), HashMap::new());
            }
            if let FieldType::Vector { dim } = field.field_type {
                vector_maps.insert(field.name.clone(), ExactVectorIndex::new(dim));
            }
        }

        CollectionIndexes {
            primary_field,
            unique_fields,
            secondary_fields,
            unique_maps,
            secondary_maps,
            vector_maps,
        }
    }

    /// Whether an equality index exists for `field`.
    pub fn has_equality_index(&self, field: &str) -> bool {
        self.unique_maps.contains_key(field) || self.secondary_maps.contains_key(field)
    }

    /// The primary key field name, if any.
    pub fn primary_field(&self) -> Option<&str> {
        self.primary_field.as_deref()
    }

    /// Check that inserting `record` would not violate a uniqueness constraint.
    /// `existing` is the id currently occupying the same primary key, if this is
    /// an update of an existing record (so it does not conflict with itself).
    pub fn check_unique(&self, record: &Record, replacing: Option<RecordId>) -> Result<()> {
        for field in &self.unique_fields {
            if let Some(value) = record.fields.get(field) {
                if value.is_null() {
                    continue;
                }
                let key = index_key(value);
                if let Some(existing) = self.unique_maps[field].get(&key) {
                    if Some(*existing) != replacing && *existing != record.id {
                        return Err(Error::UniqueViolation(format!(
                            "field {field} value already exists"
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    /// Insert `record` into all indexes. Call [`check_unique`](Self::check_unique)
    /// first; this method assumes constraints are satisfied.
    pub fn insert(&mut self, record: &Record) {
        for (field, map) in self.unique_maps.iter_mut() {
            if let Some(value) = record.fields.get(field) {
                if !value.is_null() {
                    map.insert(index_key(value), record.id);
                }
            }
        }
        for (field, map) in self.secondary_maps.iter_mut() {
            if let Some(value) = record.fields.get(field) {
                if !value.is_null() {
                    map.entry(index_key(value)).or_default().push(record.id);
                }
            }
        }
        for (field, idx) in self.vector_maps.iter_mut() {
            if let Some(Value::Vector(v)) = record.fields.get(field) {
                idx.insert(record.id, v.clone());
            }
        }
    }

    /// Remove `record` from all indexes.
    pub fn remove(&mut self, record: &Record) {
        for (field, map) in self.unique_maps.iter_mut() {
            if let Some(value) = record.fields.get(field) {
                let key = index_key(value);
                if map.get(&key) == Some(&record.id) {
                    map.remove(&key);
                }
            }
        }
        for (field, map) in self.secondary_maps.iter_mut() {
            if let Some(value) = record.fields.get(field) {
                if let Some(ids) = map.get_mut(&index_key(value)) {
                    ids.retain(|id| *id != record.id);
                }
            }
        }
        for idx in self.vector_maps.values_mut() {
            idx.remove(record.id);
        }
    }

    /// Look up record ids for an equality match on `field`, if an index exists.
    /// Returns `None` when no index covers the field (caller falls back to scan).
    pub fn lookup_eq(&self, field: &str, value: &Value) -> Option<Vec<RecordId>> {
        let key = index_key(value);
        if let Some(map) = self.unique_maps.get(field) {
            return Some(map.get(&key).copied().into_iter().collect());
        }
        if let Some(map) = self.secondary_maps.get(field) {
            return Some(map.get(&key).cloned().unwrap_or_default());
        }
        None
    }

    /// Resolve a primary key value to a record id.
    pub fn primary_lookup(&self, value: &Value) -> Option<RecordId> {
        let field = self.primary_field.as_ref()?;
        self.unique_maps.get(field)?.get(&index_key(value)).copied()
    }

    /// Whether `field` has a vector index, and its expected dimension.
    pub fn vector_dim(&self, field: &str) -> Option<usize> {
        self.vector_maps.get(field).map(|idx| idx.dim())
    }

    /// Exact nearest-neighbour search over a vector field.
    pub fn vector_nearest(
        &self,
        field: &str,
        query: &[f32],
        k: usize,
        metric: Metric,
    ) -> Result<Vec<Neighbor>> {
        let idx = self
            .vector_maps
            .get(field)
            .ok_or_else(|| Error::InvalidRequest(format!("no vector index on field {field}")))?;
        if query.len() != idx.dim() {
            return Err(Error::InvalidRequest(format!(
                "query vector dimension {} does not match field dimension {}",
                query.len(),
                idx.dim()
            )));
        }
        Ok(idx.nearest(query, k, metric))
    }

    /// Rebuild all indexes from a fresh set of records (used on open / rebuild).
    pub fn rebuild<'a>(&mut self, records: impl Iterator<Item = &'a Record>) -> Result<()> {
        for map in self.unique_maps.values_mut() {
            map.clear();
        }
        for map in self.secondary_maps.values_mut() {
            map.clear();
        }
        for idx in self.vector_maps.values_mut() {
            *idx = ExactVectorIndex::new(idx.dim());
        }
        for record in records {
            self.check_unique(record, None)?;
            self.insert(record);
        }
        Ok(())
    }

    /// Verify that the indexes are consistent with a record map: every indexed
    /// field of every record resolves back to that record. Used by `auradb check`.
    pub fn consistency_check<'a>(
        &self,
        records: impl Iterator<Item = &'a Record>,
    ) -> Result<usize> {
        let mut checked = 0;
        for record in records {
            for field in self
                .unique_fields
                .iter()
                .chain(self.secondary_fields.iter())
            {
                if let Some(value) = record.fields.get(field) {
                    if value.is_null() {
                        continue;
                    }
                    let ids = self.lookup_eq(field, value).unwrap_or_default();
                    if !ids.contains(&record.id) {
                        return Err(Error::Corruption(format!(
                            "index for {field} is missing record {}",
                            record.id
                        )));
                    }
                }
            }
            checked += 1;
        }
        Ok(checked)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auradb_core::{CollectionId, Document, FieldDef, FieldType};

    fn schema() -> CollectionSchema {
        CollectionSchema::new("Doc")
            .with_field(FieldDef {
                name: "id".into(),
                field_type: FieldType::Uuid,
                primary_key: true,
                unique: true,
                nullable: false,
                indexed: false,
            })
            .with_field(FieldDef {
                name: "email".into(),
                field_type: FieldType::String,
                primary_key: false,
                unique: true,
                nullable: true,
                indexed: false,
            })
            .with_field(FieldDef {
                name: "status".into(),
                field_type: FieldType::String,
                primary_key: false,
                unique: false,
                nullable: true,
                indexed: true,
            })
            .with_field(FieldDef::new("embedding", FieldType::Vector { dim: 3 }))
    }

    fn record(id: u128, email: &str, status: &str, vec: Vec<f32>) -> Record {
        let mut f = Document::new();
        f.insert("id".into(), Value::Text(format!("id-{id}")));
        f.insert("email".into(), Value::Text(email.into()));
        f.insert("status".into(), Value::Text(status.into()));
        f.insert("embedding".into(), Value::Vector(vec));
        Record::new(RecordId::from_u128(id), CollectionId::new("Doc"), f)
    }

    #[test]
    fn primary_and_secondary_lookup() {
        let mut idx = CollectionIndexes::from_schema(&schema());
        let r = record(1, "a@x.com", "published", vec![1.0, 0.0, 0.0]);
        idx.check_unique(&r, None).unwrap();
        idx.insert(&r);
        assert_eq!(
            idx.primary_lookup(&Value::Text("id-1".into())),
            Some(RecordId::from_u128(1))
        );
        assert_eq!(
            idx.lookup_eq("status", &Value::Text("published".into())),
            Some(vec![RecordId::from_u128(1)])
        );
        assert_eq!(idx.lookup_eq("title", &Value::Text("x".into())), None);
    }

    #[test]
    fn unique_violation_detected() {
        let mut idx = CollectionIndexes::from_schema(&schema());
        let r1 = record(1, "dup@x.com", "a", vec![1.0, 0.0, 0.0]);
        idx.insert(&r1);
        let r2 = record(2, "dup@x.com", "b", vec![0.0, 1.0, 0.0]);
        assert!(matches!(
            idx.check_unique(&r2, None),
            Err(Error::UniqueViolation(_))
        ));
    }

    #[test]
    fn update_does_not_conflict_with_self() {
        let mut idx = CollectionIndexes::from_schema(&schema());
        let r1 = record(1, "a@x.com", "a", vec![1.0, 0.0, 0.0]);
        idx.insert(&r1);
        // Same id, same unique email - replacing itself is fine.
        let updated = record(1, "a@x.com", "b", vec![1.0, 0.0, 0.0]);
        idx.check_unique(&updated, Some(RecordId::from_u128(1)))
            .unwrap();
    }

    #[test]
    fn delete_removes_index_entry() {
        let mut idx = CollectionIndexes::from_schema(&schema());
        let r = record(1, "a@x.com", "published", vec![1.0, 0.0, 0.0]);
        idx.insert(&r);
        idx.remove(&r);
        assert_eq!(
            idx.lookup_eq("status", &Value::Text("published".into())),
            Some(vec![])
        );
        assert_eq!(idx.primary_lookup(&Value::Text("id-1".into())), None);
    }

    #[test]
    fn vector_nearest_orders_by_similarity() {
        let mut idx = CollectionIndexes::from_schema(&schema());
        idx.insert(&record(1, "a@x.com", "x", vec![1.0, 0.0, 0.0]));
        idx.insert(&record(2, "b@x.com", "x", vec![0.0, 1.0, 0.0]));
        idx.insert(&record(3, "c@x.com", "x", vec![0.9, 0.1, 0.0]));
        let result = idx
            .vector_nearest("embedding", &[1.0, 0.0, 0.0], 2, Metric::Cosine)
            .unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].id, RecordId::from_u128(1));
        assert_eq!(result[1].id, RecordId::from_u128(3));
    }

    #[test]
    fn vector_dimension_mismatch_rejected() {
        let idx = CollectionIndexes::from_schema(&schema());
        assert!(idx
            .vector_nearest("embedding", &[1.0, 0.0], 1, Metric::Cosine)
            .is_err());
    }

    #[test]
    fn rebuild_and_consistency_check() {
        let recs = [
            record(1, "a@x.com", "p", vec![1.0, 0.0, 0.0]),
            record(2, "b@x.com", "d", vec![0.0, 1.0, 0.0]),
        ];
        let mut idx = CollectionIndexes::from_schema(&schema());
        idx.rebuild(recs.iter()).unwrap();
        assert_eq!(idx.consistency_check(recs.iter()).unwrap(), 2);
    }
}
