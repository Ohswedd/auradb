//! Command implementations for the `auradb` CLI. Kept separate from `main.rs`
//! so each command can be unit-tested without spawning a process.
#![forbid(unsafe_code)]

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use auradb::core::{CollectionSchema, Document, Value};
use auradb::query::{FindQuery, Mutation};
use auradb::Engine;
use auradb_server::{Config, Server};
use serde::{Deserialize, Serialize};

/// The package version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// `auradb version`
pub fn cmd_version() -> String {
    format!("auradb {VERSION}")
}

/// `auradb init` - create the data directory and write a default config file.
pub fn cmd_init(data_dir: &Path, config_path: &Path) -> Result<()> {
    std::fs::create_dir_all(data_dir)
        .with_context(|| format!("creating data dir {}", data_dir.display()))?;
    // Opening the engine initializes the manifest, catalog, and first segment.
    Engine::open(data_dir).context("initializing storage")?;
    let config = Config {
        data_dir: data_dir.to_path_buf(),
        ..Config::default()
    };
    std::fs::write(config_path, config.to_toml())
        .with_context(|| format!("writing config {}", config_path.display()))?;
    Ok(())
}

/// `auradb doctor` - validate config and data directory and report stats.
pub fn cmd_doctor(data_dir: &Path, config: &Config) -> Result<String> {
    config.validate().context("config validation")?;
    let mut report = String::new();
    report.push_str(&format!("data_dir: {}\n", data_dir.display()));
    report.push_str(&format!("exists: {}\n", data_dir.exists()));
    let engine = Engine::open(data_dir).context("opening engine")?;
    let stats = engine.stats();
    report.push_str(&format!("collections: {}\n", stats.collections));
    report.push_str(&format!("records: {}\n", stats.records));
    report.push_str(&format!("schema_version: {}\n", stats.schema_version));
    let checked = engine.check_consistency().context("consistency check")?;
    report.push_str(&format!(
        "index_consistency: ok ({checked} records verified)\n"
    ));
    Ok(report)
}

/// `auradb check` - verify index consistency.
pub fn cmd_check(data_dir: &Path) -> Result<String> {
    let engine = Engine::open(data_dir)?;
    let checked = engine.check_consistency()?;
    Ok(format!("index consistency OK; {checked} records verified"))
}

/// `auradb compact` - compact storage.
pub fn cmd_compact(data_dir: &Path) -> Result<String> {
    let engine = Engine::open(data_dir)?;
    let report = engine.compact()?;
    Ok(format!(
        "compacted {} segment(s) into {}; {} live records retained",
        report.segments_before, report.segments_after, report.live_records
    ))
}

/// A line in a dump file: either a schema or a record.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum DumpLine {
    Schema {
        schema: CollectionSchema,
    },
    Record {
        collection: String,
        fields: Document,
    },
}

/// `auradb dump` - export schemas and records as JSONL. Returns the line count.
pub fn cmd_dump(data_dir: &Path, out: &Path) -> Result<usize> {
    let engine = Engine::open(data_dir)?;
    let file = std::fs::File::create(out)
        .with_context(|| format!("creating dump file {}", out.display()))?;
    let mut writer = std::io::BufWriter::new(file);
    let mut lines = 0;
    let schemas = engine.list_schemas();
    for schema in &schemas {
        let line = DumpLine::Schema {
            schema: schema.clone(),
        };
        serde_json::to_writer(&mut writer, &line)?;
        writer.write_all(b"\n")?;
        lines += 1;
    }
    for schema in &schemas {
        for row in engine.find(&FindQuery::new(&schema.name))? {
            let line = DumpLine::Record {
                collection: schema.name.clone(),
                fields: row.fields,
            };
            serde_json::to_writer(&mut writer, &line)?;
            writer.write_all(b"\n")?;
            lines += 1;
        }
    }
    writer.flush()?;
    Ok(lines)
}

/// `auradb restore` - load schemas and records from a JSONL dump. Returns the
/// number of records restored.
pub fn cmd_restore(data_dir: &Path, input: &Path) -> Result<usize> {
    let engine = Engine::open(data_dir)?;
    let file = std::fs::File::open(input)
        .with_context(|| format!("opening dump file {}", input.display()))?;
    let reader = std::io::BufReader::new(file);
    let mut records = 0;
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let parsed: DumpLine = serde_json::from_str(&line).context("parsing dump line")?;
        match parsed {
            DumpLine::Schema { schema } => {
                engine.create_schema(schema)?;
            }
            DumpLine::Record { collection, fields } => {
                engine.apply_mutation(Mutation::Upsert { collection, fields })?;
                records += 1;
            }
        }
    }
    Ok(records)
}

/// `auradb server` - start the network server until Ctrl-C.
pub async fn cmd_server(config: Config) -> Result<()> {
    auradb_observability::init_tracing(&config.log_level, config.log_json);
    let server = Server::open(config).context("opening server")?;
    server
        .run(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .context("server run")?;
    Ok(())
}

/// `auradb status` - connect to a running server and report health.
pub async fn cmd_status(addr: &str) -> Result<String> {
    let mut client = auradb_conformance::Client::connect(addr)
        .await
        .with_context(|| format!("connecting to {addr}"))?;
    client.ping().await.context("ping")?;
    let health = client.health().await.context("health")?;
    Ok(format!(
        "status: {:?}\nready: {}\nversion: {}\ncollections: {}",
        health.status, health.ready, health.version, health.collections
    ))
}

/// `auradb bench` - run a local insert/read/vector benchmark.
pub fn cmd_bench(data_dir: &Path, records: usize) -> Result<String> {
    use auradb::core::{FieldDef, FieldType};
    use auradb::query::VectorSearch;

    let engine = Engine::open(data_dir)?;
    let schema = CollectionSchema::new("Bench")
        .with_field(FieldDef {
            name: "id".into(),
            field_type: FieldType::Uuid,
            primary_key: true,
            unique: true,
            nullable: false,
            indexed: false,
        })
        .with_field(FieldDef::new("embedding", FieldType::Vector { dim: 8 }));
    engine.create_schema(schema)?;

    let write_start = Instant::now();
    for i in 0..records {
        let mut f = Document::new();
        f.insert("id".into(), Value::Text(format!("b{i}")));
        let v: Vec<f32> = (0..8).map(|j| ((i + j) % 17) as f32).collect();
        f.insert("embedding".into(), Value::Vector(v));
        engine.insert("Bench", f)?;
    }
    let write_elapsed = write_start.elapsed();

    let read_start = Instant::now();
    let rows = engine.find(&FindQuery::new("Bench"))?;
    let read_elapsed = read_start.elapsed();

    let vec_start = Instant::now();
    let mut q = FindQuery::new("Bench");
    q.vector = Some(VectorSearch {
        field: "embedding".into(),
        query: vec![1.0; 8],
        k: 10,
        metric: "cosine".into(),
    });
    let _ = engine.find(&q)?;
    let vec_elapsed = vec_start.elapsed();

    let writes_per_sec = records as f64 / write_elapsed.as_secs_f64().max(1e-9);
    Ok(format!(
        "bench results ({records} records):\n  inserts: {:.0} ops/s ({:?} total)\n  full scan: {} rows in {:?}\n  vector top-10: {:?}",
        writes_per_sec, write_elapsed, rows.len(), read_elapsed, vec_elapsed
    ))
}

/// Build a [`Config`] from an optional file plus CLI overrides.
pub fn build_config(
    config_path: Option<&Path>,
    data_dir: Option<PathBuf>,
    bind: Option<String>,
    port: Option<u16>,
) -> Result<Config> {
    let mut config = match config_path {
        Some(path) => Config::load(path).with_context(|| format!("loading {}", path.display()))?,
        None => Config::default(),
    };
    if let Some(d) = data_dir {
        config.data_dir = d;
    }
    if let Some(b) = bind {
        config.bind = b;
    }
    if let Some(p) = port {
        config.port = p;
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_creates_dir_and_config() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let cfg = dir.path().join("AuraDB.toml");
        cmd_init(&data, &cfg).unwrap();
        assert!(data.exists());
        assert!(cfg.exists());
        let config = Config {
            data_dir: data.clone(),
            ..Config::default()
        };
        let report = cmd_doctor(&data, &config).unwrap();
        assert!(report.contains("index_consistency: ok"));
    }

    #[test]
    fn dump_then_restore_roundtrips() {
        use auradb::core::{FieldDef, FieldType};
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        {
            let engine = Engine::open(&src).unwrap();
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
            for i in 0..3 {
                let mut f = Document::new();
                f.insert("id".into(), Value::Text(format!("r{i}")));
                engine.insert("C", f).unwrap();
            }
        }
        let dump = dir.path().join("dump.jsonl");
        let lines = cmd_dump(&src, &dump).unwrap();
        assert_eq!(lines, 1 + 3);

        let dst = dir.path().join("dst");
        let restored = cmd_restore(&dst, &dump).unwrap();
        assert_eq!(restored, 3);
        let engine = Engine::open(&dst).unwrap();
        assert_eq!(engine.find(&FindQuery::new("C")).unwrap().len(), 3);
    }

    #[test]
    fn check_and_compact() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        cmd_init(&data, &dir.path().join("c.toml")).unwrap();
        assert!(cmd_check(&data).unwrap().contains("OK"));
        assert!(cmd_compact(&data).unwrap().contains("compacted"));
    }

    #[test]
    fn bench_runs() {
        let dir = tempfile::tempdir().unwrap();
        let out = cmd_bench(dir.path(), 50).unwrap();
        assert!(out.contains("ops/s"));
    }
}
