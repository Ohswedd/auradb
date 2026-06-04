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
pub mod persist;

use std::collections::{HashMap, HashSet};

use auradb_core::{CollectionSchema, Error, FieldType, Record, RecordId, Result, Value};

pub use metric::{metric_json, Metric};
pub use persist::{IndexManifest, IndexSnapshot, INDEX_FORMAT_VERSION};

/// Compute a content fingerprint of a collection from its records.
///
/// This is an FNV-1a hash over each record's id and version in storage scan
/// order (which is deterministic). It changes whenever any record is inserted,
/// updated, or deleted, and is used on open to detect a stale persisted index
/// snapshot relative to the current storage state.
pub fn fingerprint<'a>(records: impl Iterator<Item = &'a Record>) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for r in records {
        for b in r.id.to_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        for b in r.version.to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

/// A canonical, hashable key derived from a [`Value`] for equality indexing.
fn index_key(value: &Value) -> String {
    serde_json::to_string(&value.to_json()).unwrap_or_default()
}

/// Tokenize text for full-text indexing and search.
///
/// Tokens are case-folded (lowercased) and split on every non-alphanumeric
/// boundary (punctuation and whitespace). No stop-word removal is applied: every
/// token is indexed and searchable. This keeps semantics predictable; callers
/// that want stop-word filtering can preprocess their text.
pub fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// A simple in-memory inverted index over the tokens of one text field.
#[derive(Debug, Default)]
struct TextIndex {
    /// term -> (record id -> term frequency within this field).
    postings: HashMap<String, HashMap<RecordId, u32>>,
}

impl TextIndex {
    fn add(&mut self, id: RecordId, text: &str) {
        for term in tokenize(text) {
            *self
                .postings
                .entry(term)
                .or_default()
                .entry(id)
                .or_insert(0) += 1;
        }
    }

    fn remove(&mut self, id: RecordId, text: &str) {
        for term in tokenize(text) {
            if let Some(map) = self.postings.get_mut(&term) {
                map.remove(&id);
                if map.is_empty() {
                    self.postings.remove(&term);
                }
            }
        }
    }

    /// Boolean-AND search: a record matches when it contains every distinct
    /// query term. Results are ranked by summed term frequency (descending),
    /// tie-broken by record id.
    fn search(&self, query: &str) -> Vec<(RecordId, f32)> {
        let mut terms = tokenize(query);
        terms.sort();
        terms.dedup();
        if terms.is_empty() {
            return Vec::new();
        }
        let mut matched: HashMap<RecordId, (u32, f32)> = HashMap::new();
        for term in &terms {
            if let Some(map) = self.postings.get(term) {
                for (id, tf) in map {
                    let entry = matched.entry(*id).or_insert((0, 0.0));
                    entry.0 += 1;
                    entry.1 += *tf as f32;
                }
            }
        }
        let need = terms.len() as u32;
        let mut out: Vec<(RecordId, f32)> = matched
            .into_iter()
            .filter(|(_, (m, _))| *m == need)
            .map(|(id, (_, score))| (id, score))
            .collect();
        out.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.0.cmp(&b.0))
        });
        out
    }
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
    /// dotted document path -> value key -> record ids.
    doc_path_maps: HashMap<String, HashMap<String, Vec<RecordId>>>,
    /// text field name -> inverted index.
    text_maps: HashMap<String, TextIndex>,
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
        let mut doc_path_maps = HashMap::new();
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

        for path in schema.document_path_indexes() {
            doc_path_maps
                .entry(path.to_string())
                .or_insert_with(HashMap::new);
        }

        let mut text_maps = HashMap::new();
        for field in schema.full_text_indexes() {
            text_maps
                .entry(field.to_string())
                .or_insert_with(TextIndex::default);
        }

        CollectionIndexes {
            primary_field,
            unique_fields,
            secondary_fields,
            unique_maps,
            secondary_maps,
            doc_path_maps,
            text_maps,
            vector_maps,
        }
    }

    /// Whether an equality index exists for `field` (a field name or a dotted
    /// document path).
    pub fn has_equality_index(&self, field: &str) -> bool {
        self.unique_maps.contains_key(field)
            || self.secondary_maps.contains_key(field)
            || self.doc_path_maps.contains_key(field)
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
        for (path, map) in self.doc_path_maps.iter_mut() {
            if let Some(value) = record.get_path(path) {
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
        for (field, ti) in self.text_maps.iter_mut() {
            if let Some(Value::Text(s)) = record.fields.get(field) {
                ti.add(record.id, s);
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
        for (path, map) in self.doc_path_maps.iter_mut() {
            if let Some(value) = record.get_path(path) {
                if let Some(ids) = map.get_mut(&index_key(value)) {
                    ids.retain(|id| *id != record.id);
                }
            }
        }
        for idx in self.vector_maps.values_mut() {
            idx.remove(record.id);
        }
        for (field, ti) in self.text_maps.iter_mut() {
            if let Some(Value::Text(s)) = record.fields.get(field) {
                ti.remove(record.id, s);
            }
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
        if let Some(map) = self.doc_path_maps.get(field) {
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

    /// Whether `field` has a full-text index.
    pub fn has_text_index(&self, field: &str) -> bool {
        self.text_maps.contains_key(field)
    }

    /// Full-text search over a text-indexed field. Returns matching record ids
    /// with a simple term-frequency score, ranked highest first.
    pub fn text_search(&self, field: &str, query: &str) -> Result<Vec<(RecordId, f32)>> {
        let ti = self
            .text_maps
            .get(field)
            .ok_or_else(|| Error::InvalidRequest(format!("no full-text index on field {field}")))?;
        Ok(ti.search(query))
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

    /// Produce a serializable snapshot of these indexes for persistence.
    pub fn snapshot(&self, schema_version: u64, fingerprint: u64) -> IndexSnapshot {
        let unique = self
            .unique_maps
            .iter()
            .map(|(field, map)| persist::UniqueIndexData {
                field: field.clone(),
                entries: map.iter().map(|(k, id)| (k.clone(), *id)).collect(),
            })
            .collect();
        let secondary = self
            .secondary_maps
            .iter()
            .map(|(field, map)| persist::SecondaryIndexData {
                field: field.clone(),
                entries: map
                    .iter()
                    .map(|(k, ids)| (k.clone(), ids.clone()))
                    .collect(),
            })
            .collect();
        let document_paths = self
            .doc_path_maps
            .iter()
            .map(|(path, map)| persist::SecondaryIndexData {
                field: path.clone(),
                entries: map
                    .iter()
                    .map(|(k, ids)| (k.clone(), ids.clone()))
                    .collect(),
            })
            .collect();
        let vectors = self
            .vector_maps
            .iter()
            .map(|(field, idx)| persist::VectorIndexData {
                field: field.clone(),
                dim: idx.dim(),
                entries: idx.entries.iter().map(|(id, v)| (*id, v.clone())).collect(),
            })
            .collect();
        IndexSnapshot {
            format_version: INDEX_FORMAT_VERSION,
            schema_version,
            fingerprint,
            primary_field: self.primary_field.clone(),
            unique,
            secondary,
            document_paths,
            vectors,
            text: self
                .text_maps
                .iter()
                .map(|(field, ti)| persist::TextIndexData {
                    field: field.clone(),
                    postings: ti
                        .postings
                        .iter()
                        .map(|(term, m)| {
                            (term.clone(), m.iter().map(|(id, tf)| (*id, *tf)).collect())
                        })
                        .collect(),
                })
                .collect(),
        }
    }

    /// Reconstruct collection indexes from a persisted snapshot, validating that
    /// its field shape matches `schema`. Returns an error (so the caller rebuilds
    /// from storage) if the snapshot is shape-incompatible.
    pub fn from_snapshot(schema: &CollectionSchema, snapshot: IndexSnapshot) -> Result<Self> {
        let mut idx = CollectionIndexes::from_schema(schema);
        if idx.primary_field != snapshot.primary_field {
            return Err(Error::Corruption(
                "index snapshot primary key does not match schema".into(),
            ));
        }

        let expect_unique: HashSet<&str> = idx.unique_maps.keys().map(String::as_str).collect();
        let got_unique: HashSet<&str> = snapshot.unique.iter().map(|u| u.field.as_str()).collect();
        let expect_secondary: HashSet<&str> =
            idx.secondary_maps.keys().map(String::as_str).collect();
        let got_secondary: HashSet<&str> = snapshot
            .secondary
            .iter()
            .map(|s| s.field.as_str())
            .collect();
        let expect_vector: HashSet<&str> = idx.vector_maps.keys().map(String::as_str).collect();
        let got_vector: HashSet<&str> = snapshot.vectors.iter().map(|v| v.field.as_str()).collect();
        let expect_doc_path: HashSet<&str> = idx.doc_path_maps.keys().map(String::as_str).collect();
        let got_doc_path: HashSet<&str> = snapshot
            .document_paths
            .iter()
            .map(|s| s.field.as_str())
            .collect();
        let expect_text: HashSet<&str> = idx.text_maps.keys().map(String::as_str).collect();
        let got_text: HashSet<&str> = snapshot.text.iter().map(|t| t.field.as_str()).collect();
        if expect_unique != got_unique
            || expect_secondary != got_secondary
            || expect_vector != got_vector
            || expect_doc_path != got_doc_path
            || expect_text != got_text
        {
            return Err(Error::Corruption(
                "index snapshot fields do not match schema".into(),
            ));
        }

        for u in snapshot.unique {
            let map = idx.unique_maps.get_mut(&u.field).expect("checked above");
            map.extend(u.entries);
        }
        for s in snapshot.secondary {
            let map = idx.secondary_maps.get_mut(&s.field).expect("checked above");
            map.extend(s.entries);
        }
        for d in snapshot.document_paths {
            let map = idx.doc_path_maps.get_mut(&d.field).expect("checked above");
            map.extend(d.entries);
        }
        for t in snapshot.text {
            let ti = idx.text_maps.get_mut(&t.field).expect("checked above");
            for (term, posting) in t.postings {
                ti.postings.entry(term).or_default().extend(posting);
            }
        }
        for v in snapshot.vectors {
            let vi = idx.vector_maps.get_mut(&v.field).expect("checked above");
            if vi.dim() != v.dim {
                return Err(Error::Corruption(format!(
                    "index snapshot vector dimension mismatch on field {}",
                    v.field
                )));
            }
            for (id, vector) in v.entries {
                vi.insert(id, vector);
            }
        }
        Ok(idx)
    }

    /// Rebuild all indexes from a fresh set of records (used on open / rebuild).
    pub fn rebuild<'a>(&mut self, records: impl Iterator<Item = &'a Record>) -> Result<()> {
        for map in self.unique_maps.values_mut() {
            map.clear();
        }
        for map in self.secondary_maps.values_mut() {
            map.clear();
        }
        for map in self.doc_path_maps.values_mut() {
            map.clear();
        }
        for ti in self.text_maps.values_mut() {
            *ti = TextIndex::default();
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
            for path in self.doc_path_maps.keys() {
                if let Some(value) = record.get_path(path) {
                    if value.is_null() {
                        continue;
                    }
                    let ids = self.lookup_eq(path, value).unwrap_or_default();
                    if !ids.contains(&record.id) {
                        return Err(Error::Corruption(format!(
                            "document-path index for {path} is missing record {}",
                            record.id
                        )));
                    }
                }
            }
            for (field, ti) in &self.text_maps {
                if let Some(Value::Text(s)) = record.fields.get(field) {
                    for term in tokenize(s) {
                        let present = ti
                            .postings
                            .get(&term)
                            .is_some_and(|m| m.contains_key(&record.id));
                        if !present {
                            return Err(Error::Corruption(format!(
                                "full-text index for {field} is missing record {}",
                                record.id
                            )));
                        }
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

    #[test]
    fn snapshot_roundtrips_via_disk() {
        let dir = tempfile::tempdir().unwrap();
        let recs = [
            record(1, "a@x.com", "p", vec![1.0, 0.0, 0.0]),
            record(2, "b@x.com", "d", vec![0.0, 1.0, 0.0]),
        ];
        let mut idx = CollectionIndexes::from_schema(&schema());
        idx.rebuild(recs.iter()).unwrap();
        let fp = fingerprint(recs.iter());
        let snap = idx.snapshot(7, fp);
        let path = dir.path().join("c.idx");
        persist::write_snapshot(&path, &snap).unwrap();

        let loaded = persist::read_snapshot(&path).unwrap();
        assert_eq!(loaded.fingerprint, fp);
        let idx2 = CollectionIndexes::from_snapshot(&schema(), loaded).unwrap();
        assert_eq!(
            idx2.primary_lookup(&Value::Text("id-1".into())),
            Some(RecordId::from_u128(1))
        );
        assert_eq!(
            idx2.lookup_eq("status", &Value::Text("p".into())),
            Some(vec![RecordId::from_u128(1)])
        );
        let nn = idx2
            .vector_nearest("embedding", &[1.0, 0.0, 0.0], 1, Metric::Cosine)
            .unwrap();
        assert_eq!(nn[0].id, RecordId::from_u128(1));
    }

    #[test]
    fn corrupt_index_file_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        let mut idx = CollectionIndexes::from_schema(&schema());
        idx.insert(&record(1, "a@x.com", "p", vec![1.0, 0.0, 0.0]));
        let path = dir.path().join("c.idx");
        persist::write_snapshot(&path, &idx.snapshot(1, 0)).unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        std::fs::write(&path, &bytes).unwrap();
        assert!(matches!(
            persist::read_snapshot(&path),
            Err(Error::Corruption(_))
        ));
    }

    #[test]
    fn snapshot_schema_mismatch_is_rejected() {
        let mut idx = CollectionIndexes::from_schema(&schema());
        idx.insert(&record(1, "a@x.com", "p", vec![1.0, 0.0, 0.0]));
        let snap = idx.snapshot(1, 0);
        let other = CollectionSchema::new("Doc").with_field(FieldDef {
            name: "id".into(),
            field_type: FieldType::Uuid,
            primary_key: true,
            unique: true,
            nullable: false,
            indexed: false,
        });
        assert!(CollectionIndexes::from_snapshot(&other, snap).is_err());
    }
}
