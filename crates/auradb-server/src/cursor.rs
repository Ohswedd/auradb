//! Server-side cursors.
//!
//! A cursor holds the ordered *ids* of a planned query (not full rows) plus a
//! position, so memory is bounded by the result-id count rather than the
//! materialized payload. Rows are materialized per page on demand. Cursors time
//! out after an idle interval and are reaped; they are also closed when their
//! owning connection disconnects.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use auradb::query::{FindQuery, Row};
use auradb::{Engine, Transaction};
use auradb_core::{Error, RecordId, Result};

struct CursorEntry {
    query: FindQuery,
    ordered: Vec<(RecordId, Option<f32>)>,
    position: usize,
    last_used: Instant,
}

/// A page returned from a cursor fetch.
pub struct CursorPage {
    /// The rows in this page.
    pub rows: Vec<Row>,
    /// Whether more rows remain (the cursor is still open).
    pub more: bool,
}

/// A registry of open server-side cursors.
pub struct CursorRegistry {
    inner: Mutex<Registry>,
    timeout: Duration,
}

struct Registry {
    next_id: u64,
    cursors: HashMap<u64, CursorEntry>,
}

impl CursorRegistry {
    /// Create a registry with the given idle timeout.
    pub fn new(timeout: Duration) -> Self {
        CursorRegistry {
            inner: Mutex::new(Registry {
                next_id: 1,
                cursors: HashMap::new(),
            }),
            timeout,
        }
    }

    /// Open a cursor over a planned query result, returning its id.
    pub fn open(&self, query: FindQuery, ordered: Vec<(RecordId, Option<f32>)>) -> u64 {
        let mut reg = self.inner.lock().expect("cursor registry poisoned");
        let id = reg.next_id;
        reg.next_id += 1;
        reg.cursors.insert(
            id,
            CursorEntry {
                query,
                ordered,
                position: 0,
                last_used: Instant::now(),
            },
        );
        id
    }

    /// Fetch up to `limit` rows from a cursor, materializing them via `engine`.
    /// The cursor is automatically closed when exhausted.
    pub fn fetch(&self, id: u64, limit: usize, engine: &Engine) -> Result<CursorPage> {
        self.fetch_with(id, limit, engine, None)
    }

    /// Fetch a page, materializing rows against a transaction view when `txn`
    /// is supplied. A cursor opened inside a transaction must materialize its
    /// pages through the same transaction so that staged writes remain visible
    /// and staged deletes stay hidden across paging.
    pub fn fetch_with(
        &self,
        id: u64,
        limit: usize,
        engine: &Engine,
        txn: Option<&Transaction>,
    ) -> Result<CursorPage> {
        let (query, page_ids, more) = {
            let mut reg = self.inner.lock().expect("cursor registry poisoned");
            let entry = reg
                .cursors
                .get_mut(&id)
                .ok_or_else(|| Error::NotFound(format!("cursor {id}")))?;
            entry.last_used = Instant::now();
            let end = (entry.position + limit.max(1)).min(entry.ordered.len());
            let page_ids = entry.ordered[entry.position..end].to_vec();
            entry.position = end;
            let more = entry.position < entry.ordered.len();
            (entry.query.clone(), page_ids, more)
        };
        let rows = match txn {
            Some(txn) => engine.txn_materialize(txn, &query, &page_ids)?,
            None => engine.materialize(&query, &page_ids)?,
        };
        if !more {
            self.close(id);
        }
        Ok(CursorPage { rows, more })
    }

    /// Close a cursor, returning whether it existed.
    pub fn close(&self, id: u64) -> bool {
        let mut reg = self.inner.lock().expect("cursor registry poisoned");
        reg.cursors.remove(&id).is_some()
    }

    /// Reap cursors idle longer than the timeout, returning how many were
    /// removed.
    pub fn reap(&self) -> usize {
        let mut reg = self.inner.lock().expect("cursor registry poisoned");
        let timeout = self.timeout;
        let before = reg.cursors.len();
        reg.cursors.retain(|_, c| c.last_used.elapsed() < timeout);
        before - reg.cursors.len()
    }

    /// The number of open cursors.
    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .expect("cursor registry poisoned")
            .cursors
            .len()
    }

    /// Whether there are no open cursors.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auradb::core::{CollectionSchema, Document, FieldDef, FieldType, Value};

    fn engine_with_rows(n: usize) -> (tempfile::TempDir, Engine) {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::open(dir.path()).unwrap();
        engine
            .create_schema(CollectionSchema::new("C").with_field(FieldDef {
                name: "id".into(),
                field_type: FieldType::Uuid,
                primary_key: true,
                unique: true,
                nullable: false,
                indexed: false,
            }))
            .unwrap();
        for i in 0..n {
            let mut f = Document::new();
            f.insert("id".into(), Value::Text(format!("r{i}")));
            engine.insert("C", f).unwrap();
        }
        (dir, engine)
    }

    #[test]
    fn paging_through_a_cursor() {
        let (_dir, engine) = engine_with_rows(5);
        let planned = engine.plan_find(&FindQuery::new("C")).unwrap();
        let reg = CursorRegistry::new(Duration::from_secs(60));
        let id = reg.open(FindQuery::new("C"), planned.ordered);

        let p1 = reg.fetch(id, 2, &engine).unwrap();
        assert_eq!(p1.rows.len(), 2);
        assert!(p1.more);
        let p2 = reg.fetch(id, 2, &engine).unwrap();
        assert_eq!(p2.rows.len(), 2);
        assert!(p2.more);
        let p3 = reg.fetch(id, 2, &engine).unwrap();
        assert_eq!(p3.rows.len(), 1);
        assert!(!p3.more);
        // Auto-closed at exhaustion.
        assert!(reg.fetch(id, 2, &engine).is_err());
    }

    #[test]
    fn explicit_close() {
        let (_dir, engine) = engine_with_rows(3);
        let planned = engine.plan_find(&FindQuery::new("C")).unwrap();
        let reg = CursorRegistry::new(Duration::from_secs(60));
        let id = reg.open(FindQuery::new("C"), planned.ordered);
        assert!(reg.close(id));
        assert!(!reg.close(id));
        assert!(reg.is_empty());
    }

    #[test]
    fn timeout_reaping() {
        let (_dir, engine) = engine_with_rows(3);
        let planned = engine.plan_find(&FindQuery::new("C")).unwrap();
        let reg = CursorRegistry::new(Duration::from_millis(0));
        let _id = reg.open(FindQuery::new("C"), planned.ordered);
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(reg.reap(), 1);
        assert!(reg.is_empty());
    }
}
