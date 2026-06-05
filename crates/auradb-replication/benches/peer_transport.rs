//! Peer-transport microbenchmarks: the encode/decode cost of a Raft message as
//! it crosses the cluster wire (JSON serialization plus a CRC32 over the
//! payload, matching the framing in `auradb-replication`'s peer transport).
//!
//! These measure the dominant per-message cost on the replication path. They use
//! only public `auradb-raft` types so the benchmark tracks the same data the
//! transport actually serializes.

use auradb_raft::{Command, CommandKind, LogEntry, LogIndex, Message, Term};
use criterion::{criterion_group, criterion_main, BatchSize, Criterion};

/// Build a representative AppendEntries carrying `n` data entries.
fn append_entries(n: usize) -> Message {
    let entries = (1..=n as u64)
        .map(|i| LogEntry {
            term: Term(2),
            index: LogIndex(i),
            command: Command::new(CommandKind::Database, vec![0xAB; 256]),
        })
        .collect();
    Message::AppendEntries {
        term: Term(2),
        prev_log_index: LogIndex(0),
        prev_log_term: Term(0),
        entries,
        leader_commit: LogIndex(0),
    }
}

fn encode(msg: &Message) -> Vec<u8> {
    let payload = serde_json::to_vec(msg).expect("serialize");
    let _crc = crc32fast::hash(&payload);
    payload
}

fn decode(bytes: &[u8]) -> Message {
    let _crc = crc32fast::hash(bytes);
    serde_json::from_slice(bytes).expect("deserialize")
}

fn bench_peer_transport(c: &mut Criterion) {
    let heartbeat = append_entries(0);
    let batch = append_entries(64);

    c.bench_function("peer_encode_heartbeat", |b| {
        b.iter(|| encode(std::hint::black_box(&heartbeat)))
    });
    c.bench_function("peer_encode_batch_64", |b| {
        b.iter(|| encode(std::hint::black_box(&batch)))
    });

    let heartbeat_bytes = encode(&heartbeat);
    let batch_bytes = encode(&batch);
    c.bench_function("peer_decode_heartbeat", |b| {
        b.iter_batched(
            || heartbeat_bytes.clone(),
            |bytes| decode(std::hint::black_box(&bytes)),
            BatchSize::SmallInput,
        )
    });
    c.bench_function("peer_decode_batch_64", |b| {
        b.iter_batched(
            || batch_bytes.clone(),
            |bytes| decode(std::hint::black_box(&bytes)),
            BatchSize::SmallInput,
        )
    });
}

criterion_group!(benches, bench_peer_transport);
criterion_main!(benches);
