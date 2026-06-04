# Architecture Decisions

## ADR-1: Append-only log storage with rebuilt in-memory indexes

**Decision.** The first-release storage engine is an append-only segment log.
Each record mutation (put / delete tombstone) is appended as a length-prefixed,
CRC32C-checksummed envelope. On open, the engine replays segments to rebuild the
live record map and the secondary indexes.

**Why.** It is simple, crash-safe (a torn tail record is detected by checksum and
truncated), and honest. It satisfies durability, recovery, and corruption
detection without claiming a B-tree/page cache we have not implemented.

**Boundary.** Physical offsets are never exposed as stable identity; records are
addressed by stable `RecordId`. Offset-pointer optimization is future work.

## ADR-2: Single-writer staged transactions

**Decision.** Transactions stage their write set in memory. Commit acquires the
engine write lock, performs optimistic write-write conflict detection against the
versions read at transaction start, then appends a commit batch to the log
atomically (commit marker written last). Rollback discards the staged set.

**Why.** This gives real atomicity, durability, read-your-writes, and rollback on
a single node with a documented isolation level (snapshot reads + optimistic
conflict detection on commit). We do not claim serializable MVCC.

## ADR-3: AWP frames carry JSON payloads

**Decision.** The Aura Wire Protocol header is binary with a fixed header and
CRC32C header plus payload checksums. Message payloads
(query IR, results, errors) are encoded as JSON.

**Why.** JSON payloads keep the Query IR transparent and debuggable and let the
conformance harness and any future `aura-connector` build interoperate against a
documented schema. The binary framing provides the efficient transport,
versioning, and checksums the protocol requires. A future change can swap payload
encoding for a compact binary form without changing the frame layout.

## ADR-4: Exact vector search only

**Decision.** Vector fields are validated for fixed dimension and stored inline.
Nearest-neighbour search is exact (full scan with cosine / euclidean / dot
ranking) behind a `VectorIndex` trait.

**Why.** Exact search is correct and testable now. The trait leaves room for an
HNSW implementation later without changing the query engine. We do not claim ANN
performance.

## ADR-5: `auradb` as the embeddable engine

**Decision.** The `auradb` crate composes storage, index, txn, and query into a
single `Engine` with a synchronous API. `auradb-server` wraps it for the network.

**Why.** Keeps the network layer thin and lets the engine be unit-tested and
embedded directly (and reused by the CLI and benches).
