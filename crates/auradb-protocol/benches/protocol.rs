//! Benchmarks for AWP frame encoding and decoding.

use auradb_protocol::{Frame, Opcode, RequestId, DEFAULT_MAX_PAYLOAD};
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn bench_frame(c: &mut Criterion) {
    let payload = serde_json::to_vec(&serde_json::json!({
        "query": "find",
        "collection": "Doc",
        "filter": {"type": "compare", "field": "status", "op": "eq", "value": "published"},
    }))
    .unwrap();
    let frame = Frame::new(Opcode::Query, RequestId(42), 0, payload);

    c.bench_function("frame_encode", |b| b.iter(|| black_box(frame.encode())));

    let bytes = frame.encode();
    c.bench_function("frame_decode", |b| {
        b.iter(|| {
            let decoded = Frame::decode(black_box(&bytes), DEFAULT_MAX_PAYLOAD)
                .unwrap()
                .unwrap();
            black_box(decoded)
        })
    });
}

criterion_group!(benches, bench_frame);
criterion_main!(benches);
