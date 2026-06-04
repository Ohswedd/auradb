# AuraDB Architecture

AuraDB is a single-node multi-model database server. It speaks the Aura Wire
Protocol (AWP) and executes an Aura-Connector-compatible Query IR over a
persistent append-only storage engine.

## Layering

```
auradb-core        types, ids, values, schema, errors, capabilities
    │
auradb-protocol    AWP frame encode/decode + message payloads
auradb-storage     append-only log, manifest, schema catalog, recovery
auradb-index       primary/unique/secondary indexes, exact vector search
auradb-txn         staged write sets, commit/rollback, conflict detection
    │
auradb-query       Query IR + executor + EXPLAIN + migration estimate
    │
auradb             Engine: composes storage + index + txn + query + schema
    │
auradb-observability   tracing, metrics, health/readiness
auradb-server      TCP listener, dispatch, server-side cursors, config
    │
auradb-cli         operator CLI
auradb-conformance protocol client + conformance scenarios (test crate)
```

## Request lifecycle

1. A client opens a TCP connection and sends a `HELLO` frame; the server
   negotiates the protocol version and replies with capabilities.
2. Each request is one AWP frame with a JSON payload and a request id.
3. The server decodes the frame, validates checksums and payload limits, and
   dispatches by opcode to the engine.
4. Query / mutation opcodes run through `auradb-query` against the engine.
5. Results are streamed directly or paged through a server-side cursor.
6. Every response carries the originating request id; errors are structured
   `ERROR` frames with stable codes.

## Durability and recovery

The storage engine appends checksummed record envelopes to numbered segment
files described by a manifest. Committed transactions write a commit marker after
their record batch. On startup the engine replays segments, ignoring any
unfinished trailing batch and truncating a torn tail detected by checksum, then
rebuilds in-memory indexes from the live records.

See `docs/STORAGE_ENGINE.md`, `docs/TRANSACTIONS.md`, and `docs/PROTOCOL.md`.
