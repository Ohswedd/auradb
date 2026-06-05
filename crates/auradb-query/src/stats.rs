//! Planner statistics: per-collection summaries the query planner uses to
//! estimate access-path cost.
//!
//! Statistics are computed from a collection's records (`auradb analyze` /
//! [`CollectionStats::compute`]) and persisted to `planner_stats.json` so the
//! planner makes the same choices across restarts. They are *advisory*: a missing
//! or stale stats file never changes query results, only the chosen plan. When no
//! statistics are available the planner falls back to live counts and default
//! selectivity assumptions.

use std::collections::BTreeMap;
use std::path::Path;

use auradb_core::{CollectionSchema, FieldType, Record, Result, Value};
use serde::{Deserialize, Serialize};

/// The persisted planner-stats format version.
pub const STATS_FORMAT_VERSION: u32 = 1;

/// A canonical, hashable string for a value (matches the index keying scheme),
/// used for distinct-value counting.
fn value_key(value: &Value) -> String {
    serde_json::to_string(&value.to_json()).unwrap_or_default()
}

/// Summary statistics for one collection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectionStats {
    /// Number of live records.
    pub row_count: usize,
    /// Distinct non-null value count per indexed field or document path. Higher
    /// cardinality means a more selective equality lookup.
    #[serde(default)]
    pub field_cardinality: BTreeMap<String, usize>,
    /// Number of indexed vectors per vector field.
    #[serde(default)]
    pub vector_count: BTreeMap<String, usize>,
    /// Number of records with a non-empty value per full-text field.
    #[serde(default)]
    pub text_field_docs: BTreeMap<String, usize>,
    /// Average serialized record size in bytes (0 when unknown).
    #[serde(default)]
    pub avg_record_size: usize,
}

impl CollectionStats {
    /// Compute statistics for `schema` from its records.
    pub fn compute<'a>(
        schema: &CollectionSchema,
        records: impl Iterator<Item = &'a Record>,
    ) -> CollectionStats {
        // Which fields/paths we track cardinality for: indexed scalar fields plus
        // document-path indexes.
        let mut cardinality_fields: Vec<String> = Vec::new();
        let mut vector_fields: Vec<String> = Vec::new();
        for field in &schema.fields {
            if field.primary_key || field.unique || field.indexed {
                cardinality_fields.push(field.name.clone());
            }
            if matches!(field.field_type, FieldType::Vector { .. }) {
                vector_fields.push(field.name.clone());
            }
        }
        let doc_paths: Vec<String> = schema.document_path_indexes().map(str::to_string).collect();
        cardinality_fields.extend(doc_paths.iter().cloned());
        let text_fields: Vec<String> = schema.full_text_indexes().map(str::to_string).collect();

        let mut distinct: BTreeMap<String, std::collections::HashSet<String>> = BTreeMap::new();
        let mut vector_count: BTreeMap<String, usize> = BTreeMap::new();
        let mut text_field_docs: BTreeMap<String, usize> = BTreeMap::new();
        let mut row_count = 0usize;
        let mut total_size = 0usize;

        for record in records {
            row_count += 1;
            total_size += serde_json::to_vec(&record.fields)
                .map(|v| v.len())
                .unwrap_or(0);
            for field in &cardinality_fields {
                if let Some(value) = record.get_path(field) {
                    if !value.is_null() {
                        distinct
                            .entry(field.clone())
                            .or_default()
                            .insert(value_key(value));
                    }
                }
            }
            for field in &vector_fields {
                if let Some(Value::Vector(_)) = record.fields.get(field) {
                    *vector_count.entry(field.clone()).or_default() += 1;
                }
            }
            for field in &text_fields {
                if let Some(Value::Text(s)) = record.fields.get(field) {
                    if !s.trim().is_empty() {
                        *text_field_docs.entry(field.clone()).or_default() += 1;
                    }
                }
            }
        }

        let field_cardinality = distinct
            .into_iter()
            .map(|(k, set)| (k, set.len()))
            .collect();
        let avg_record_size = total_size.checked_div(row_count).unwrap_or(0);

        CollectionStats {
            row_count,
            field_cardinality,
            vector_count,
            text_field_docs,
            avg_record_size,
        }
    }

    /// Distinct value count for `field`, if known.
    pub fn cardinality(&self, field: &str) -> Option<usize> {
        self.field_cardinality.get(field).copied()
    }
}

/// All persisted planner statistics, keyed by collection name.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannerStats {
    /// Stats format version.
    pub format_version: u32,
    /// Per-collection statistics.
    #[serde(default)]
    pub collections: BTreeMap<String, CollectionStats>,
}

impl Default for PlannerStats {
    fn default() -> Self {
        PlannerStats {
            format_version: STATS_FORMAT_VERSION,
            collections: BTreeMap::new(),
        }
    }
}

impl PlannerStats {
    /// Statistics for one collection, if present.
    pub fn get(&self, collection: &str) -> Option<&CollectionStats> {
        self.collections.get(collection)
    }

    /// Load planner stats from `path`. Statistics are advisory, so a missing,
    /// unreadable, malformed, or version-mismatched file yields empty stats
    /// rather than an error (the planner falls back to live estimates).
    pub fn load(path: &Path) -> PlannerStats {
        let Ok(bytes) = std::fs::read(path) else {
            return PlannerStats::default();
        };
        match serde_json::from_slice::<PlannerStats>(&bytes) {
            Ok(stats) if stats.format_version == STATS_FORMAT_VERSION => stats,
            _ => PlannerStats::default(),
        }
    }

    /// Persist planner stats atomically to `path` (write temp + rename).
    pub fn save(&self, path: &Path) -> Result<()> {
        let tmp = path.with_extension("tmp");
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|e| auradb_core::Error::Storage(format!("stats serialization failed: {e}")))?;
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
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
                name: "status".into(),
                field_type: FieldType::String,
                primary_key: false,
                unique: false,
                nullable: true,
                indexed: true,
            })
    }

    fn rec(id: u128, status: &str) -> Record {
        let mut f = Document::new();
        f.insert("id".into(), Value::Text(format!("id{id}")));
        f.insert("status".into(), Value::Text(status.into()));
        Record::new(
            auradb_core::RecordId::from_u128(id),
            CollectionId::new("Doc"),
            f,
        )
    }

    #[test]
    fn computes_row_count_and_cardinality() {
        let recs = [rec(1, "a"), rec(2, "a"), rec(3, "b")];
        let stats = CollectionStats::compute(&schema(), recs.iter());
        assert_eq!(stats.row_count, 3);
        assert_eq!(stats.cardinality("status"), Some(2)); // a, b
        assert_eq!(stats.cardinality("id"), Some(3)); // all distinct
    }

    #[test]
    fn persists_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("planner_stats.json");
        let mut stats = PlannerStats::default();
        stats.collections.insert(
            "Doc".into(),
            CollectionStats::compute(&schema(), [rec(1, "a")].iter()),
        );
        stats.save(&path).unwrap();
        let back = PlannerStats::load(&path);
        assert_eq!(back.get("Doc").unwrap().row_count, 1);
    }

    #[test]
    fn missing_stats_file_is_empty_not_error() {
        let stats = PlannerStats::load(Path::new("/nonexistent/planner_stats.json"));
        assert!(stats.collections.is_empty());
    }
}
