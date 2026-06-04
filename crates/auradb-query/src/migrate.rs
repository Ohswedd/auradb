//! Migration impact estimation.
//!
//! Given a proposed target schema, estimate the impact of applying it against
//! the current data and schema: fields added/removed, new indexes that would be
//! built, and how many existing records would fail validation under the new
//! schema. This is a local, read-only estimate - it does not modify anything.

use auradb_core::{CollectionSchema, FieldType, Result};
use serde::{Deserialize, Serialize};

use crate::exec::DataSource;

/// The estimated impact of applying a target schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MigrationEstimate {
    /// The collection being migrated.
    pub collection: String,
    /// Whether the collection already exists.
    pub exists: bool,
    /// Number of existing records in the collection.
    pub records_affected: usize,
    /// Field names present in the target but not the current schema.
    pub added_fields: Vec<String>,
    /// Field names present in the current schema but not the target.
    pub removed_fields: Vec<String>,
    /// New equality (unique/indexed/primary) indexes that would be built.
    pub new_indexes: Vec<String>,
    /// Vector indexes that would be (re)built.
    pub vector_indexes_to_build: Vec<String>,
    /// Number of existing records that would fail validation under the target.
    pub records_failing_validation: usize,
    /// Whether applying the migration requires a full scan of existing data.
    pub requires_full_scan: bool,
    /// Advisory warnings.
    pub warnings: Vec<String>,
}

/// Estimate the impact of migrating a collection to `target`.
pub fn estimate(ds: &dyn DataSource, target: &CollectionSchema) -> Result<MigrationEstimate> {
    target.validate_definition()?;
    let current = ds.schema(&target.name);
    let exists = current.is_some();

    let current_field_names: Vec<String> = current
        .map(|s| s.fields.iter().map(|f| f.name.clone()).collect())
        .unwrap_or_default();
    let target_field_names: Vec<String> = target.fields.iter().map(|f| f.name.clone()).collect();

    let added_fields: Vec<String> = target_field_names
        .iter()
        .filter(|n| !current_field_names.contains(n))
        .cloned()
        .collect();
    let removed_fields: Vec<String> = current_field_names
        .iter()
        .filter(|n| !target_field_names.contains(n))
        .cloned()
        .collect();

    let mut new_indexes = Vec::new();
    let mut vector_indexes_to_build = Vec::new();
    for field in &target.fields {
        let was_indexed = current
            .and_then(|s| s.field(&field.name))
            .map(|f| f.primary_key || f.unique || f.indexed)
            .unwrap_or(false);
        if (field.primary_key || field.unique || field.indexed) && !was_indexed {
            new_indexes.push(field.name.clone());
        }
        if matches!(field.field_type, FieldType::Vector { .. }) {
            vector_indexes_to_build.push(field.name.clone());
        }
    }

    // Count existing records that would fail validation under the target schema.
    let mut records_affected = 0;
    let mut records_failing_validation = 0;
    for record in ds.scan(&target.name) {
        records_affected += 1;
        if target.validate_record(&record.fields).is_err() {
            records_failing_validation += 1;
        }
    }

    let mut warnings = Vec::new();
    if records_failing_validation > 0 {
        warnings.push(format!(
            "{records_failing_validation} existing record(s) would fail validation under the new schema"
        ));
    }
    if !removed_fields.is_empty() {
        warnings.push(format!(
            "dropping fields {:?} discards their stored values on rewrite",
            removed_fields
        ));
    }

    let requires_full_scan =
        !new_indexes.is_empty() || !vector_indexes_to_build.is_empty() || records_affected > 0;

    Ok(MigrationEstimate {
        collection: target.name.clone(),
        exists,
        records_affected,
        added_fields,
        removed_fields,
        new_indexes,
        vector_indexes_to_build,
        records_failing_validation,
        requires_full_scan,
        warnings,
    })
}
