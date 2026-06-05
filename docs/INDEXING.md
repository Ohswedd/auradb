# Indexing

`auradb-index` provides in-memory indexes for each collection, rebuilt from
storage on open and kept consistent on every mutation.

## Index kinds

- **Primary key** - a unique map from the primary-key value to the record id.
- **Unique** - per-field maps enforcing uniqueness on insert and update.
- **Secondary** - per-field equality maps for fields marked `indexed`.
- **Document-path** - equality maps on a nested document value addressed by a
  dotted path, declared via a schema `indexes` entry
  `{ "path": "profile.company", "kind": "document_path" }`. See
  [DOCUMENTS.md](DOCUMENTS.md).
- **Full-text** - a tokenized inverted index on a string field, declared via
  `{ "path": "body", "kind": "full_text" }`. See [FULL_TEXT.md](FULL_TEXT.md).
- **Vector** - an exact (brute-force) index per vector field, behind the
  `VectorIndex` trait.

Equality keys are canonical JSON strings of the value, giving consistent
equality semantics across types.

## Lifecycle

- **Build/rebuild** - on engine open and on `create_schema`, indexes are rebuilt
  by scanning the collection's records; uniqueness is validated during rebuild.
- **Insert/update/delete** - the engine updates indexes in lockstep with the
  storage write (remove old entries, insert new), under the write lock.
- **Consistency check** - `check_consistency` (and `auradb check`) verifies every
  indexed field of every record resolves back to that record.

## Query use

The query planner uses an equality index to seed candidate selection when a
top-level `eq` filter targets an indexed field, including a document-path index
for a dotted-path equality (EXPLAIN reports `strategy: index_lookup` and the
`used_index`). A `contains_text` filter uses a full-text index when present
(EXPLAIN reports `strategy: full_text_scan`). Otherwise the planner falls back to
a full scan (and EXPLAIN warns on large scans). Ranges, ordering, and `contains`
are evaluated over candidates rather than via the index. The planner chooses the
most selective applicable index by estimated cost; see
[QUERY_ENGINE.md](QUERY_ENGINE.md).

## MVCC visibility

Indexes map values to record **ids**, not to specific versions. The executor
resolves those ids through MVCC visibility (Option A): the DataSource or
transaction view applies snapshot (as-of) or latest-committed visibility, so an
index never surfaces an invisible version. Because indexes reflect the latest live
state and version GC always keeps the latest version of every live record,
**indexes need no rebuild after GC**. See [STORAGE_ENGINE.md](STORAGE_ENGINE.md)
and [TRANSACTIONS.md](TRANSACTIONS.md).

## Persistence model

Indexes are held in memory and also snapshotted to disk so they are not always
rebuilt on open.

### Snapshot layout

Snapshots live in an `indexes/` directory under the data dir: an
`INDEX_MANIFEST.json` and per-collection `.idx` files. Each `.idx` file has a
framed header: a 4-byte magic `AIDX`, a `u32` format version, a `u32` payload
length, a `u32` CRC32 of the payload, then a JSON payload. Persisted kinds are
primary key, unique, secondary, document-path, full-text, and exact vector.

### Checkpoints

Snapshots are written at checkpoints: `auradb compact`, graceful server
shutdown, and `auradb index rebuild`.

### Load and staleness detection

On open, the engine loads a snapshot only when all of the following hold:

- its content fingerprint (an FNV-1a hash over each record's id and version)
  matches the current storage state,
- the snapshot's index field shape matches the schema, and
- the CRC is valid.

Otherwise the engine safely rebuilds the index from storage and records that it
rebuilt. A crash between checkpoints is detected on the next open as a
fingerprint mismatch and triggers a rebuild; queries never return incorrect
results from a stale snapshot.

### Inspecting and rebuilding

`auradb index check` reports how indexes loaded (how many from a snapshot, how
many rebuilt) and verifies consistency. `auradb index rebuild` rebuilds from
storage and persists fresh snapshots. `auradb check` and `auradb compact` also
validate and preserve indexes.

## Vector indexes

Vector search is exact: the index scans stored vectors and ranks by the chosen
metric. The `VectorIndex` trait leaves room for an approximate (ANN) index such
as HNSW as future work without changing the query engine. ANN is **not** claimed
in this release.

## Tests

Primary/secondary lookup, unique violation, update-vs-self, delete removes
entry, rebuild + consistency check, vector nearest ordering, and dimension
enforcement.
