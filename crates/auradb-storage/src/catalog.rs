//! The schema catalog: durable storage of collection schemas.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use auradb_core::{CollectionSchema, Error, Result};
use serde::{Deserialize, Serialize};

/// The persisted catalog of collection schemas.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    /// Collection name -> schema.
    pub schemas: BTreeMap<String, CollectionSchema>,
    /// Monotonic schema version, bumped on each change.
    #[serde(default)]
    pub schema_version: u64,
}

impl Catalog {
    /// Load the catalog from `path`, returning an empty catalog if absent.
    pub fn load(path: &Path) -> Result<Catalog> {
        if !path.exists() {
            return Ok(Catalog::default());
        }
        let bytes = fs::read(path)?;
        serde_json::from_slice(&bytes)
            .map_err(|e| Error::Corruption(format!("schema catalog is malformed: {e}")))
    }

    /// Atomically persist the catalog to `path`.
    pub fn save(&self, path: &Path) -> Result<()> {
        let tmp = path.with_extension("tmp");
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|e| Error::Storage(format!("catalog serialization failed: {e}")))?;
        fs::write(&tmp, &bytes)?;
        let f = fs::File::open(&tmp)?;
        f.sync_all()?;
        fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Insert or replace a schema, bumping the schema version.
    pub fn put(&mut self, schema: CollectionSchema) {
        self.schema_version += 1;
        self.schemas.insert(schema.name.clone(), schema);
    }

    /// Remove a schema by name, returning whether it existed.
    pub fn remove(&mut self, name: &str) -> bool {
        let existed = self.schemas.remove(name).is_some();
        if existed {
            self.schema_version += 1;
        }
        existed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auradb_core::{FieldDef, FieldType};

    #[test]
    fn catalog_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let mut cat = Catalog::default();
        cat.put(CollectionSchema::new("User").with_field(FieldDef::new("id", FieldType::Uuid)));
        cat.save(&path).unwrap();
        let back = Catalog::load(&path).unwrap();
        assert_eq!(cat, back);
        assert_eq!(back.schema_version, 1);
    }

    #[test]
    fn missing_catalog_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let cat = Catalog::load(&dir.path().join("nope.json")).unwrap();
        assert!(cat.schemas.is_empty());
    }
}
