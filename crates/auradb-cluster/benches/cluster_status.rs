//! Benchmark for assembling a cluster status snapshot.

use std::hint::black_box;

use auradb_cluster::{ClusterStatus, ClusterStore};
use criterion::{criterion_group, criterion_main, Criterion};
use tempfile::tempdir;

fn bench_status(c: &mut Criterion) {
    let dir = tempdir().unwrap();
    let identity = ClusterStore::new(dir.path())
        .init(None, None, "0.4.0")
        .unwrap();
    c.bench_function("cluster_status_snapshot", |b| {
        b.iter(|| {
            let status = ClusterStatus::idle_single_node(black_box(&identity));
            black_box(status.replication_lag_entries());
        });
    });
}

criterion_group!(benches, bench_status);
criterion_main!(benches);
