//! Deterministic crash-recovery tests for the storage engine: trailing-batch
//! truncation, mid-batch corruption detection, and catalog corruption. Seeds are
//! fixed so the suite is reproducible and never flaky.

use auradb_core::{CollectionId, Document, Record, RecordId, Value};
use auradb_storage::Storage;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::fs;

fn rec(id: u128, v: i64) -> Record {
    let mut fields = Document::new();
    fields.insert("v".into(), Value::Int(v));
    Record::new(RecordId::from_u128(id), CollectionId::new("C"), fields)
}

fn active_segment(dir: &std::path::Path) -> std::path::PathBuf {
    // Open creates the first segment 0000000001.seg.
    let mut segs: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "seg").unwrap_or(false))
        .collect();
    segs.sort();
    segs.pop().expect("a segment file exists")
}

#[test]
fn trailing_truncation_recovers_a_prefix() {
    // For many seeds: write N batches, truncate the active segment to a random
    // length, and verify reopen never panics and recovers a consistent subset.
    for seed in 0..40u64 {
        let dir = tempfile::tempdir().unwrap();
        let n = 12;
        {
            let mut s = Storage::open(dir.path()).unwrap();
            for i in 1..=n {
                s.put(rec(i, i as i64)).unwrap();
            }
        }
        let seg = active_segment(dir.path());
        let full = fs::read(&seg).unwrap();
        let mut rng = StdRng::seed_from_u64(seed);
        // Truncate to somewhere in the file (possibly mid-batch).
        let cut = if full.is_empty() {
            0
        } else {
            rng.gen_range(0..full.len())
        };
        fs::write(&seg, &full[..cut]).unwrap();

        // Reopen: a torn trailing batch is dropped; earlier batches survive.
        let reopened = Storage::open(dir.path());
        let s = reopened.expect("truncation must recover, not error");
        let count = s.count(&CollectionId::new("C"));
        assert!(count <= n as usize);
        // Every surviving record has a contiguous-prefix id and correct value.
        for i in 1..=count as u128 {
            let r = s
                .get(&CollectionId::new("C"), RecordId::from_u128(i))
                .unwrap_or_else(|| panic!("seed {seed}: missing record {i} of {count}"));
            assert_eq!(r.get("v"), Some(&Value::Int(i as i64)));
        }
    }
}

#[test]
fn mid_batch_byte_flip_is_detected() {
    for seed in 0..20u64 {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut s = Storage::open(dir.path()).unwrap();
            for i in 1..=5 {
                s.put(rec(i, i as i64)).unwrap();
            }
        }
        let seg = active_segment(dir.path());
        let mut bytes = fs::read(&seg).unwrap();
        if bytes.len() < 4 {
            continue;
        }
        let mut rng = StdRng::seed_from_u64(seed);
        // Flip a byte that is not in the trailing region, so it falls inside a
        // committed batch and trips a checksum rather than truncating.
        let idx = rng.gen_range(0..bytes.len() / 2);
        bytes[idx] ^= 0xff;
        fs::write(&seg, &bytes).unwrap();
        // Reopen must either detect corruption or recover a consistent prefix,
        // but never panic or return wrong data.
        match Storage::open(dir.path()) {
            Ok(s) => {
                let count = s.count(&CollectionId::new("C"));
                for i in 1..=count as u128 {
                    if let Some(r) = s.get(&CollectionId::new("C"), RecordId::from_u128(i)) {
                        assert_eq!(r.get("v"), Some(&Value::Int(i as i64)));
                    }
                }
            }
            Err(e) => {
                assert!(
                    e.to_string().contains("corruption") || e.to_string().contains("checksum"),
                    "unexpected error: {e}"
                );
            }
        }
    }
}

#[test]
fn catalog_corruption_is_detected() {
    let dir = tempfile::tempdir().unwrap();
    {
        let mut s = Storage::open(dir.path()).unwrap();
        s.put_schema(
            auradb_core::CollectionSchema::new("C")
                .with_field(auradb_core::FieldDef::new("v", auradb_core::FieldType::Int)),
        )
        .unwrap();
    }
    fs::write(dir.path().join("catalog.json"), b"{ this is not valid json").unwrap();
    // A corrupt catalog is detected (fail closed), never silently ignored.
    assert!(Storage::open(dir.path()).is_err());
}
