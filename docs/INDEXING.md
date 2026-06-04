# Indexing

`auradb-index` provides in-memory indexes for each collection, rebuilt from
storage on open and kept consistent on every mutation.

## Index kinds

- **Primary key** - a unique map from the primary-key value to the record id.
- **Unique** - per-field maps enforcing uniqueness on insert and update.
- **Secondary** - per-field equality maps for fields marked `indexed`.
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
top-level `eq` filter targets an indexed field; otherwise it falls back to a full
scan (and EXPLAIN warns on large scans). Ranges, ordering, and `contains` are
evaluated over candidates rather than via the index.

## Persistence model

Indexes are in-memory and rebuilt from the durable record log on startup; they
are not separately persisted. This is correct and simple for the first release
and is documented as such. A persisted/incremental index build is future work.

## Vector indexes

Vector search is exact: the index scans stored vectors and ranks by the chosen
metric. The `VectorIndex` trait leaves room for an approximate (ANN) index such
as HNSW as future work without changing the query engine. ANN is **not** claimed
in this release.

## Tests

Primary/secondary lookup, unique violation, update-vs-self, delete removes
entry, rebuild + consistency check, vector nearest ordering, and dimension
enforcement.
