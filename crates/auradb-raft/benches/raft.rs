//! Benchmarks for the Raft log: single-node append and durable log recovery.

use std::hint::black_box;

use auradb_cluster::NodeId;
use auradb_raft::{
    single_node, Command, CommandKind, FileStorage, LogEntry, LogIndex, RaftStorage, Term,
};
use criterion::{criterion_group, criterion_main, Criterion};
use tempfile::tempdir;

fn db_command(i: u64) -> Command {
    Command::new(CommandKind::Database, i.to_be_bytes().to_vec())
}

fn bench_single_node_append(c: &mut Criterion) {
    c.bench_function("raft_single_node_propose", |b| {
        let dir = tempdir().unwrap();
        let mut node = single_node(NodeId::from_raw(1), FileStorage::open(dir.path()).unwrap());
        node.campaign();
        let mut i = 0u64;
        b.iter(|| {
            i += 1;
            black_box(node.propose(db_command(i)).unwrap());
        });
    });
}

fn bench_log_recovery(c: &mut Criterion) {
    c.bench_function("raft_log_recovery_1k", |b| {
        let dir = tempdir().unwrap();
        {
            let mut s = FileStorage::open(dir.path()).unwrap();
            for i in 1..=1000u64 {
                s.append(&[LogEntry {
                    term: Term(1),
                    index: LogIndex(i),
                    command: db_command(i),
                }])
                .unwrap();
            }
        }
        b.iter(|| {
            let s = FileStorage::open(dir.path()).unwrap();
            black_box(s.last_index());
        });
    });
}

criterion_group!(benches, bench_single_node_append, bench_log_recovery);
criterion_main!(benches);
