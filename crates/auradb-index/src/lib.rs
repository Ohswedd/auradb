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

pub mod analyzer;
pub mod hnsw;
mod metric;
pub mod persist;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use auradb_core::{CollectionSchema, Error, FieldType, Record, RecordId, Result, Value};

pub use analyzer::{Analyzer, AnalyzerPreset, Token, TokenStream};
pub use hnsw::{Hnsw, HnswParams};
pub use metric::{metric_json, Metric};
pub use persist::{HnswMetadata, IndexManifest, IndexSnapshot, INDEX_FORMAT_VERSION};

/// Cache key for a per-field approximate (HNSW) graph: field name, metric name,
/// and the construction parameters. A graph is rebuilt when any of these — or the
/// underlying vectors — change.
type AnnCacheKey = (String, &'static str, usize, usize);

/// A fixed seed for the approximate-index graph so a given vector set + params
/// builds a reproducible graph (the preview is deterministic).
const ANN_GRAPH_SEED: u64 = 0x4155_5241_4442_5631; // "AURADBV1"

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

/// The default BM25 term-saturation parameter `k1`.
pub const BM25_DEFAULT_K1: f32 = 1.2;
/// The default BM25 length-normalization parameter `b`.
pub const BM25_DEFAULT_B: f32 = 0.75;

/// Summary statistics for one full-text index, surfaced by `auradb index check`
/// and `EXPLAIN` for ranked text search.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextIndexStats {
    /// Number of indexed documents (records with a non-empty value for the field).
    pub documents: usize,
    /// Number of distinct terms in the inverted index.
    pub distinct_terms: usize,
    /// Average document length in tokens (0.0 when there are no documents).
    pub avg_doc_len: f32,
}

/// A simple in-memory inverted index over the tokens of one text field.
///
/// In addition to `term -> (id -> term frequency)` postings, the index tracks the
/// token length of every indexed document and a running total, which are the
/// statistics BM25 needs for length normalization (`avgdl`) and the corpus size
/// (`N`). Document frequency per term is the length of a term's posting list.
#[derive(Debug, Default)]
struct TextIndex {
    /// term -> (record id -> term frequency within this field).
    postings: HashMap<String, HashMap<RecordId, u32>>,
    /// record id -> token length of this field for that record.
    doc_lengths: HashMap<RecordId, u32>,
    /// Sum of all document lengths (cached for `avgdl`).
    total_tokens: u64,
}

impl TextIndex {
    fn add(&mut self, id: RecordId, text: &str) {
        let mut len = 0u32;
        for term in tokenize(text) {
            *self
                .postings
                .entry(term)
                .or_default()
                .entry(id)
                .or_insert(0) += 1;
            len += 1;
        }
        if len > 0 {
            *self.doc_lengths.entry(id).or_insert(0) += len;
            self.total_tokens += len as u64;
        }
    }

    fn remove(&mut self, id: RecordId, text: &str) {
        let mut len = 0u32;
        for term in tokenize(text) {
            if let Some(map) = self.postings.get_mut(&term) {
                map.remove(&id);
                if map.is_empty() {
                    self.postings.remove(&term);
                }
            }
            len += 1;
        }
        if let Some(existing) = self.doc_lengths.get(&id).copied() {
            let dec = len.min(existing);
            self.total_tokens -= dec as u64;
            if existing <= len {
                self.doc_lengths.remove(&id);
            } else {
                self.doc_lengths.insert(id, existing - len);
            }
        }
    }

    /// The number of indexed documents (corpus size `N`).
    fn doc_count(&self) -> usize {
        self.doc_lengths.len()
    }

    /// The average document length in tokens (`avgdl`), 0.0 when empty.
    fn avg_doc_len(&self) -> f32 {
        let n = self.doc_lengths.len();
        if n == 0 {
            0.0
        } else {
            self.total_tokens as f32 / n as f32
        }
    }

    /// Recompute `doc_lengths`/`total_tokens` from the postings. Used when a
    /// persisted snapshot predates BM25 stats so the length table is missing.
    fn rebuild_lengths(&mut self) {
        let mut lengths: HashMap<RecordId, u32> = HashMap::new();
        for map in self.postings.values() {
            for (id, tf) in map {
                *lengths.entry(*id).or_insert(0) += *tf;
            }
        }
        self.total_tokens = lengths.values().map(|&l| l as u64).sum();
        self.doc_lengths = lengths;
    }

    /// Boolean-AND search: a record matches when it contains every distinct
    /// query term. Results are ranked by summed term frequency (descending),
    /// tie-broken by record id. This is the legacy `contains_text` behavior and
    /// is preserved unchanged for compatibility.
    fn search(&self, query: &str) -> Vec<(RecordId, f32)> {
        let mut terms = tokenize(query);
        terms.sort();
        terms.dedup();
        self.search_over(&self.postings, &terms)
    }

    /// Term-frequency AND search over an explicit postings view and pre-analyzed,
    /// sorted+deduped query terms. Factored out so analyzer-aware retrieval can run
    /// the identical scoring over a transformed view (see [`Self::analyzed_view`]).
    fn search_over(
        &self,
        postings: &HashMap<String, HashMap<RecordId, u32>>,
        terms: &[String],
    ) -> Vec<(RecordId, f32)> {
        if terms.is_empty() {
            return Vec::new();
        }
        let mut matched: HashMap<RecordId, (u32, f32)> = HashMap::new();
        for term in terms {
            if let Some(map) = postings.get(term) {
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

    /// Okapi BM25 ranked search. `require_all` selects AND semantics (a document
    /// must contain every query term) versus OR semantics (any term contributes).
    /// Results are ranked by descending BM25 score, tie-broken by record id, so
    /// they are deterministic. Returns an empty result for an empty query or an
    /// empty corpus.
    fn bm25_search(&self, query: &str, require_all: bool, k1: f32, b: f32) -> Vec<(RecordId, f32)> {
        let mut terms = tokenize(query);
        terms.sort();
        terms.dedup();
        self.bm25_over(&self.postings, &terms, require_all, k1, b)
    }

    /// BM25 over an explicit postings view and pre-analyzed, sorted+deduped query
    /// terms. The document-length statistics (`doc_lengths`/`total_tokens`) are
    /// always those of the default tokenization: a per-token analyzer maps each
    /// indexed token to exactly the same number of output tokens (zero for a
    /// dropped stopword), so a document's length is unchanged or a safe upper bound,
    /// and `default`/`simple` reproduce the baseline scoring byte-for-byte.
    fn bm25_over(
        &self,
        postings: &HashMap<String, HashMap<RecordId, u32>>,
        terms: &[String],
        require_all: bool,
        k1: f32,
        b: f32,
    ) -> Vec<(RecordId, f32)> {
        if terms.is_empty() {
            return Vec::new();
        }
        let n = self.doc_lengths.len() as f32;
        if n == 0.0 {
            return Vec::new();
        }
        let avgdl = (self.total_tokens as f32 / n).max(1.0);
        // (accumulated score, number of distinct query terms matched).
        let mut acc: HashMap<RecordId, (f32, u32)> = HashMap::new();
        for term in terms {
            let Some(postings) = postings.get(term) else {
                continue;
            };
            let df = postings.len() as f32;
            // BM25+ style IDF: always non-negative, avoids penalizing very common
            // terms below zero.
            let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
            for (id, tf) in postings {
                let tf = *tf as f32;
                let dl = *self.doc_lengths.get(id).unwrap_or(&0) as f32;
                let denom = tf + k1 * (1.0 - b + b * dl / avgdl);
                let contribution = idf * (tf * (k1 + 1.0)) / denom;
                let entry = acc.entry(*id).or_insert((0.0, 0));
                entry.0 += contribution;
                entry.1 += 1;
            }
        }
        let need = terms.len() as u32;
        let mut out: Vec<(RecordId, f32)> = acc
            .into_iter()
            .filter(|(_, (_, matched))| !require_all || *matched == need)
            .map(|(id, (score, _))| (id, score))
            .collect();
        out.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.0.cmp(&b.0))
        });
        out
    }

    /// Build a postings view transformed by a per-token `analyzer`: each persisted
    /// default term is mapped via [`Analyzer::map_default_token`] and the posting
    /// lists are merged under the resulting term(s). This lets a non-default
    /// analyzer (`ascii_fold`, `english_basic`, …) retrieve over the persisted
    /// default-tokenized postings without re-indexing or changing the snapshot
    /// format. Returns `None` for a non-per-token analyzer (e.g. `keyword`).
    fn analyzed_view(&self, analyzer: Analyzer) -> Option<HashMap<String, HashMap<RecordId, u32>>> {
        if !analyzer.preset().is_per_token() {
            return None;
        }
        let mut view: HashMap<String, HashMap<RecordId, u32>> = HashMap::new();
        for (term, posting) in &self.postings {
            for out in analyzer.map_default_token(term)? {
                let entry = view.entry(out).or_default();
                for (id, tf) in posting {
                    *entry.entry(*id).or_insert(0) += *tf;
                }
            }
        }
        Some(view)
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

/// Rank ordering for neighbours: better neighbours sort first (`Ordering::Less`).
/// The key is `score` descending, ties broken by `id` ascending, with the same
/// NaN handling (`partial_cmp(...).unwrap_or(Equal)`) the full sort used. Ids are
/// unique within an index, so this is a total order with no ties.
fn rank_cmp(a: &Neighbor, b: &Neighbor) -> std::cmp::Ordering {
    b.score
        .partial_cmp(&a.score)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then(a.id.cmp(&b.id))
}

/// Whether `a` is strictly a better neighbour than `b` under [`rank_cmp`].
fn better(a: &Neighbor, b: &Neighbor) -> bool {
    rank_cmp(a, b) == std::cmp::Ordering::Less
}

/// A neighbour wrapper whose `Ord` makes the **worst** neighbour the greatest, so
/// a [`std::collections::BinaryHeap`] (a max-heap) keeps the worst at its root for
/// O(1) eviction during bounded top-k selection.
struct Worst(Neighbor);

impl PartialEq for Worst {
    fn eq(&self, other: &Self) -> bool {
        rank_cmp(&self.0, &other.0) == std::cmp::Ordering::Equal
    }
}
impl Eq for Worst {}
impl PartialOrd for Worst {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Worst {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Greater == worse, so the heap root is the worst kept neighbour.
        rank_cmp(&self.0, &other.0)
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
        if k == 0 {
            return Vec::new();
        }
        // Bounded top-k selection: keep at most `k` candidates in a max-heap whose
        // *root is the worst kept neighbour* (lowest score, ties broken by highest
        // id). A new candidate that beats the current worst evicts it. This is
        // O(n log k) instead of sorting all n candidates (O(n log k)), and produces
        // the **identical** ordered top-k as the previous full sort — the ranking
        // key (`score` desc, then `id` asc) and its NaN handling are unchanged.
        let mut heap: std::collections::BinaryHeap<Worst> =
            std::collections::BinaryHeap::with_capacity(k + 1);
        for (id, v) in self.entries.iter() {
            if v.len() != query.len() {
                continue;
            }
            let cand = Neighbor {
                id: *id,
                score: metric.similarity(query, v),
                distance: metric.distance(query, v),
            };
            if heap.len() < k {
                heap.push(Worst(cand));
            } else if better(&cand, &heap.peek().expect("heap is non-empty when full").0) {
                heap.pop();
                heap.push(Worst(cand));
            }
        }
        let mut out: Vec<Neighbor> = heap.into_iter().map(|w| w.0).collect();
        out.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.id.cmp(&b.id))
        });
        out
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
    /// Lazily-built approximate (HNSW) graphs for the opt-in ANN preview, keyed by
    /// field + metric + params. Derived from `vector_maps` (never persisted) and
    /// cleared whenever the vectors change, so exact search stays the source of
    /// truth and storage format v2 is unchanged.
    ann_cache: Mutex<HashMap<AnnCacheKey, Arc<Hnsw>>>,
    /// HNSW/ANN preview lifecycle metadata loaded from the last snapshot (empty
    /// when the indexes were rebuilt fresh). Used to report durable preview state
    /// and to detect a stale generation; the graph itself is always rebuilt.
    loaded_ann_metadata: Vec<persist::HnswMetadata>,
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
            ann_cache: Mutex::new(HashMap::new()),
            loaded_ann_metadata: Vec::new(),
        }
    }

    /// The HNSW/ANN preview metadata loaded from the last snapshot, if any. Empty
    /// when the indexes were rebuilt from storage rather than loaded.
    pub fn loaded_ann_metadata(&self) -> &[persist::HnswMetadata] {
        &self.loaded_ann_metadata
    }

    /// The live HNSW/ANN preview status for each vector field: a fresh metadata
    /// record computed from the current exact-vector index, stamped with
    /// `generation`. This is the honest current state regardless of whether a
    /// graph is currently cached (graphs build lazily on first use).
    pub fn ann_preview_status(&self, generation: u64) -> Vec<persist::HnswMetadata> {
        let mut out: Vec<persist::HnswMetadata> = self
            .vector_maps
            .iter()
            .map(|(field, idx)| persist::HnswMetadata {
                field: field.clone(),
                dim: idx.dim(),
                vector_count: idx.len(),
                generation,
            })
            .collect();
        out.sort_by(|a, b| a.field.cmp(&b.field));
        out
    }

    /// Drop any cached approximate (HNSW) graphs; called whenever vectors change
    /// so the preview never serves stale results.
    fn invalidate_ann_cache(&mut self) {
        self.ann_cache.get_mut().unwrap().clear();
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
        self.invalidate_ann_cache();
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
        self.invalidate_ann_cache();
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

    /// Distinct indexed values for an equality-indexed `field`, each as a
    /// representative record id (to recover the typed value) and the number of
    /// records posted under that value. Returns `None` when `field` has no
    /// equality index, so the caller can fall back to a scan.
    ///
    /// This backs index-accelerated terms facets: the per-value counts come
    /// straight from posting-list lengths, with no record scan. Iteration order
    /// is unspecified (hash map) — callers re-sort buckets deterministically.
    pub fn facet_postings(&self, field: &str) -> Option<Vec<(RecordId, usize)>> {
        if let Some(map) = self.unique_maps.get(field) {
            // A unique index posts exactly one record per value.
            return Some(map.values().map(|id| (*id, 1)).collect());
        }
        if let Some(map) = self.secondary_maps.get(field) {
            return Some(
                map.values()
                    .filter_map(|ids| ids.first().map(|rep| (*rep, ids.len())))
                    .collect(),
            );
        }
        if let Some(map) = self.doc_path_maps.get(field) {
            return Some(
                map.values()
                    .filter_map(|ids| ids.first().map(|rep| (*rep, ids.len())))
                    .collect(),
            );
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

    /// BM25 ranked full-text search over a text-indexed field. `require_all`
    /// selects AND term semantics; otherwise any matching term contributes (OR).
    /// `k1`/`b` are the BM25 tuning parameters (see [`BM25_DEFAULT_K1`] /
    /// [`BM25_DEFAULT_B`]). Results are ranked by descending relevance and are
    /// deterministic. Errors when no full-text index covers `field`.
    pub fn text_bm25_search(
        &self,
        field: &str,
        query: &str,
        require_all: bool,
        k1: f32,
        b: f32,
    ) -> Result<Vec<(RecordId, f32)>> {
        let ti = self
            .text_maps
            .get(field)
            .ok_or_else(|| Error::InvalidRequest(format!("no full-text index on field {field}")))?;
        Ok(ti.bm25_search(query, require_all, k1, b))
    }

    /// Analyzer-aware ranked full-text search over a text-indexed field.
    ///
    /// The query and the persisted index are matched under `analyzer`'s preset:
    /// `default`/`simple` reproduce [`Self::text_bm25_search`] exactly, while a
    /// per-token preset (`ascii_fold`, `english_basic`) retrieves over a transformed
    /// view of the persisted default postings so the symmetric corpus/query analysis
    /// matches the offline `search eval` harness. `rank` selects BM25 or summed
    /// term-frequency scoring.
    ///
    /// Returns `Ok(None)` for a non-per-token analyzer (currently `keyword`), whose
    /// whole-field semantics cannot be derived from per-token postings; the caller
    /// resolves those against the stored field text instead. Errors when no
    /// full-text index covers `field`.
    // The argument list mirrors `text_bm25_search` plus the analyzer and a rank
    // selector; bundling them would only obscure the call sites in `exec.rs`.
    #[allow(clippy::too_many_arguments)]
    pub fn text_search_analyzed(
        &self,
        field: &str,
        query: &str,
        analyzer: Analyzer,
        bm25: bool,
        require_all: bool,
        k1: f32,
        b: f32,
    ) -> Result<Option<Vec<(RecordId, f32)>>> {
        let ti = self
            .text_maps
            .get(field)
            .ok_or_else(|| Error::InvalidRequest(format!("no full-text index on field {field}")))?;
        let Some(view) = ti.analyzed_view(analyzer) else {
            return Ok(None);
        };
        let mut terms = analyzer.index_terms(query);
        terms.sort();
        terms.dedup();
        let results = if bm25 {
            ti.bm25_over(&view, &terms, require_all, k1, b)
        } else {
            ti.search_over(&view, &terms)
        };
        Ok(Some(results))
    }

    /// Summary statistics for the full-text index on `field`, if one exists.
    pub fn text_index_stats(&self, field: &str) -> Option<TextIndexStats> {
        self.text_maps.get(field).map(|ti| TextIndexStats {
            documents: ti.doc_count(),
            distinct_terms: ti.postings.len(),
            avg_doc_len: ti.avg_doc_len(),
        })
    }

    /// The names of fields that have a full-text index.
    pub fn text_field_names(&self) -> impl Iterator<Item = &str> {
        self.text_maps.keys().map(String::as_str)
    }

    /// The names of fields that have a vector index, with each field's
    /// dimensionality and indexed vector count.
    pub fn vector_field_stats(&self) -> impl Iterator<Item = (&str, usize, usize)> {
        self.vector_maps
            .iter()
            .map(|(field, idx)| (field.as_str(), idx.dim(), idx.len()))
    }

    /// The number of indexed vectors on `field`, or `None` if the field has no
    /// vector index. Used to decide whether the ANN preview clears its
    /// minimum-dataset threshold.
    pub fn vector_len(&self, field: &str) -> Option<usize> {
        self.vector_maps.get(field).map(|idx| idx.len())
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

    /// Approximate nearest-neighbour search — the opt-in HNSW **preview**.
    ///
    /// Builds (and caches) a navigable graph from the field's exact vectors and
    /// queries it with beam width `ef_search`. Exact search ([`vector_nearest`])
    /// remains the correctness baseline; this trades a small, tunable amount of
    /// recall for sub-linear query cost. The graph is derived in memory and is
    /// cleared whenever the field's vectors change, so it is never stale and never
    /// persisted (storage format v2 is unchanged). Returns a structured error for
    /// an unknown field, a dimension mismatch (no vector payload is echoed), or
    /// invalid parameters.
    ///
    /// [`vector_nearest`]: Self::vector_nearest
    pub fn vector_ann_nearest(
        &self,
        field: &str,
        query: &[f32],
        k: usize,
        metric: Metric,
        params: HnswParams,
        ef_search: usize,
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
        params.validate().map_err(Error::InvalidRequest)?;

        let key: AnnCacheKey = (
            field.to_string(),
            metric.name(),
            params.m,
            params.ef_construction,
        );
        let graph = {
            let mut cache = self.ann_cache.lock().unwrap();
            if let Some(g) = cache.get(&key) {
                Arc::clone(g)
            } else {
                let entries = idx.entries.iter().map(|(id, v)| (*id, v.clone()));
                let g = Arc::new(Hnsw::build(entries, metric, params, ANN_GRAPH_SEED));
                cache.insert(key, Arc::clone(&g));
                g
            }
        };
        Ok(graph.nearest(query, k, ef_search))
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
        // Durable ANN preview metadata: one record per vector field, stamped with
        // this snapshot's generation. The graph stays in-memory/rebuilt.
        let hnsw = self.ann_preview_status(fingerprint);
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
                    doc_lengths: ti.doc_lengths.iter().map(|(id, len)| (*id, *len)).collect(),
                })
                .collect(),
            hnsw,
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
            if t.doc_lengths.is_empty() {
                // Snapshot predates BM25 stats: rebuild lengths from postings so
                // ranked search works immediately without a full index rebuild.
                ti.rebuild_lengths();
            } else {
                for (id, len) in t.doc_lengths {
                    *ti.doc_lengths.entry(id).or_insert(0) += len;
                    ti.total_tokens += len as u64;
                }
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
        // Carry the durable ANN preview metadata for status reporting. The graph
        // is not restored here — it rebuilds in memory on first preview query.
        idx.loaded_ann_metadata = snapshot.hnsw;
        Ok(idx)
    }

    /// Rebuild all indexes from a fresh set of records (used on open / rebuild).
    pub fn rebuild<'a>(&mut self, records: impl Iterator<Item = &'a Record>) -> Result<()> {
        self.invalidate_ann_cache();
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

    /// Deterministic pseudo-random vector from a seed (no RNG dependency).
    fn gen_vec(seed: u64, dim: usize) -> Vec<f32> {
        let mut s = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15).wrapping_add(1);
        (0..dim)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                ((s % 2000) as f32) / 1000.0 - 1.0
            })
            .collect()
    }

    /// The bounded top-k heap must return exactly what a full sort + truncate
    /// would, for every `k` from 0 through n, across metrics. This is the
    /// correctness guard for the O(n log k) selection optimization.
    #[test]
    fn nearest_top_k_matches_full_sort_reference() {
        let dim = 12;
        let n = 500;
        let mut idx = ExactVectorIndex::new(dim);
        for i in 0..n {
            idx.insert(
                RecordId::from_u128(i as u128 + 1),
                gen_vec(i as u64 + 1, dim),
            );
        }
        let query = gen_vec(987_654, dim);

        for metric in [Metric::Cosine, Metric::Euclidean, Metric::DotProduct] {
            // Reference: score every entry, then full-sort by (score desc, id asc).
            let mut reference: Vec<Neighbor> = idx
                .entries
                .iter()
                .map(|(id, v)| Neighbor {
                    id: *id,
                    score: metric.similarity(&query, v),
                    distance: metric.distance(&query, v),
                })
                .collect();
            reference.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(a.id.cmp(&b.id))
            });

            for k in [0usize, 1, 7, 50, n, n + 10] {
                let got = idx.nearest(&query, k, metric);
                let want: Vec<RecordId> = reference.iter().take(k).map(|nb| nb.id).collect();
                let got_ids: Vec<RecordId> = got.iter().map(|nb| nb.id).collect();
                assert_eq!(
                    got_ids, want,
                    "metric={metric:?} k={k}: heap top-k must match full sort"
                );
                // Scores must be identical too, not just the id ordering.
                for nb in &got {
                    let r = reference.iter().find(|x| x.id == nb.id).unwrap();
                    assert_eq!(nb.score, r.score);
                    assert_eq!(nb.distance, r.distance);
                }
            }
        }
    }

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

    use auradb_core::{IndexDef, IndexKind};

    fn text_schema() -> CollectionSchema {
        CollectionSchema::new("Doc")
            .with_field(FieldDef {
                name: "id".into(),
                field_type: FieldType::Uuid,
                primary_key: true,
                unique: true,
                nullable: false,
                indexed: false,
            })
            .with_field(FieldDef::new("body", FieldType::String))
            .with_index(IndexDef {
                path: "body".into(),
                kind: IndexKind::FullText,
            })
    }

    fn doc(id: u128, body: &str) -> Record {
        let mut f = Document::new();
        f.insert("id".into(), Value::Text(format!("id-{id}")));
        f.insert("body".into(), Value::Text(body.into()));
        Record::new(RecordId::from_u128(id), CollectionId::new("Doc"), f)
    }

    #[test]
    fn bm25_ranks_term_density_and_rarity() {
        let mut idx = CollectionIndexes::from_schema(&text_schema());
        // doc 1 mentions "raft" twice in a short body (high density);
        // doc 2 mentions it once amid many other words; doc 3 not at all.
        idx.insert(&doc(1, "raft consensus raft"));
        idx.insert(&doc(
            2,
            "the raft protocol coordinates many replicas across nodes",
        ));
        idx.insert(&doc(3, "storage engine compaction and flushing"));
        let out = idx
            .text_bm25_search("body", "raft", false, BM25_DEFAULT_K1, BM25_DEFAULT_B)
            .unwrap();
        assert_eq!(out.len(), 2);
        // The short, dense document ranks first.
        assert_eq!(out[0].0, RecordId::from_u128(1));
        assert_eq!(out[1].0, RecordId::from_u128(2));
        assert!(out[0].1 > out[1].1);
    }

    #[test]
    fn bm25_idf_prefers_rarer_terms() {
        let mut idx = CollectionIndexes::from_schema(&text_schema());
        idx.insert(&doc(1, "common common common rare"));
        idx.insert(&doc(2, "common common common common"));
        idx.insert(&doc(3, "common word here"));
        // "rare" appears in only one document, so it carries high IDF.
        let out = idx
            .text_bm25_search(
                "body",
                "rare common",
                false,
                BM25_DEFAULT_K1,
                BM25_DEFAULT_B,
            )
            .unwrap();
        assert_eq!(out[0].0, RecordId::from_u128(1));
    }

    #[test]
    fn bm25_and_semantics_require_all_terms() {
        let mut idx = CollectionIndexes::from_schema(&text_schema());
        idx.insert(&doc(1, "vector search engine"));
        idx.insert(&doc(2, "vector database"));
        let any = idx
            .text_bm25_search(
                "body",
                "vector engine",
                false,
                BM25_DEFAULT_K1,
                BM25_DEFAULT_B,
            )
            .unwrap();
        assert_eq!(any.len(), 2);
        let all = idx
            .text_bm25_search(
                "body",
                "vector engine",
                true,
                BM25_DEFAULT_K1,
                BM25_DEFAULT_B,
            )
            .unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].0, RecordId::from_u128(1));
    }

    #[test]
    fn bm25_empty_query_and_corpus() {
        let mut idx = CollectionIndexes::from_schema(&text_schema());
        // Empty corpus.
        assert!(idx
            .text_bm25_search("body", "anything", false, BM25_DEFAULT_K1, BM25_DEFAULT_B)
            .unwrap()
            .is_empty());
        idx.insert(&doc(1, "hello world"));
        // Empty query.
        assert!(idx
            .text_bm25_search("body", "   ", false, BM25_DEFAULT_K1, BM25_DEFAULT_B)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn bm25_missing_index_errors() {
        let idx = CollectionIndexes::from_schema(&text_schema());
        assert!(idx
            .text_bm25_search("nope", "q", false, BM25_DEFAULT_K1, BM25_DEFAULT_B)
            .is_err());
    }

    #[test]
    fn bm25_stats_rebuild_from_legacy_snapshot() {
        let mut idx = CollectionIndexes::from_schema(&text_schema());
        idx.insert(&doc(1, "alpha beta beta"));
        idx.insert(&doc(2, "beta gamma"));
        let mut snap = idx.snapshot(1, 0);
        // Simulate a pre-BM25 snapshot: drop the persisted doc lengths.
        for t in &mut snap.text {
            t.doc_lengths.clear();
        }
        let reopened = CollectionIndexes::from_snapshot(&text_schema(), snap).unwrap();
        let stats = reopened.text_index_stats("body").unwrap();
        assert_eq!(stats.documents, 2);
        // doc 1 has 3 tokens, doc 2 has 2 tokens -> avg 2.5.
        assert!((stats.avg_doc_len - 2.5).abs() < 1e-6);
        // Ranking still works after the rebuild.
        let out = reopened
            .text_bm25_search("body", "beta", false, BM25_DEFAULT_K1, BM25_DEFAULT_B)
            .unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].0, RecordId::from_u128(1));
    }

    #[test]
    fn bm25_stats_persist_across_snapshot() {
        let mut idx = CollectionIndexes::from_schema(&text_schema());
        idx.insert(&doc(1, "alpha beta beta"));
        idx.insert(&doc(2, "beta gamma delta"));
        let snap = idx.snapshot(1, 0);
        let reopened = CollectionIndexes::from_snapshot(&text_schema(), snap).unwrap();
        let stats = reopened.text_index_stats("body").unwrap();
        assert_eq!(stats.documents, 2);
        assert!((stats.avg_doc_len - 3.0).abs() < 1e-6); // (3 + 3) / 2
    }
}
