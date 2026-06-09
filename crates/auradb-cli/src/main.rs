//! The `auradb` command-line interface.
#![forbid(unsafe_code)]

use std::path::PathBuf;

use anyhow::Result;
use auradb_cli::{
    build_config, cmd_auth_hash_token, cmd_auth_rotate_token, cmd_backup_verify, cmd_bench,
    cmd_bench_compare, cmd_bench_json, cmd_cert_generate_dev, cmd_check, cmd_check_json,
    cmd_cluster_backup_plan, cmd_cluster_bootstrap, cmd_cluster_compact_log, cmd_cluster_doctor,
    cmd_cluster_doctor_live, cmd_cluster_init, cmd_cluster_leader, cmd_cluster_peers,
    cmd_cluster_restore_plan, cmd_cluster_status, cmd_cluster_status_live, cmd_cluster_wait_leader,
    cmd_cluster_wait_ready, cmd_compact, cmd_compatibility, cmd_config_validate, cmd_doctor,
    cmd_doctor_json, cmd_dump, cmd_gc, cmd_index_check, cmd_index_rebuild, cmd_init, cmd_restore,
    cmd_search_explain, cmd_server, cmd_snapshot_create, cmd_snapshot_inspect,
    cmd_snapshot_restore, cmd_stats_analyze, cmd_stats_show, cmd_status, cmd_status_json,
    cmd_vector_eval, cmd_version,
};
use clap::{Parser, Subcommand};

/// AuraDB: a single-node multi-model database server.
#[derive(Parser)]
#[command(name = "auradb", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print the version.
    Version,
    /// Initialize a data directory and write a default config file.
    Init {
        /// Data directory to create.
        #[arg(long, default_value = ".local/auradb")]
        data_dir: PathBuf,
        /// Path to write the generated config file.
        #[arg(long, default_value = "AuraDB.toml")]
        config: PathBuf,
    },
    /// Start the database server.
    Server {
        /// Optional config file.
        #[arg(long)]
        config: Option<PathBuf>,
        /// Override the data directory.
        #[arg(long)]
        data_dir: Option<PathBuf>,
        /// Override the bind address.
        #[arg(long)]
        bind: Option<String>,
        /// Override the listen port.
        #[arg(long)]
        port: Option<u16>,
        /// Permit binding a non-loopback address with authentication disabled.
        #[arg(long)]
        allow_insecure_bind: bool,
    },
    /// Print AuraDB version, protocol version, and connector compatibility.
    Compatibility,
    /// Configuration helpers.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Validate config and data directory.
    Doctor {
        #[arg(long, default_value = ".local/auradb")]
        data_dir: PathBuf,
        #[arg(long)]
        config: Option<PathBuf>,
        /// Emit the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Ping a running server and report health.
    Status {
        /// Server address (`host:port`).
        #[arg(long, default_value = "127.0.0.1:7171")]
        addr: String,
        /// Authentication token (for a server with auth enabled).
        #[arg(long)]
        token: Option<String>,
        /// PEM CA bundle to trust. Providing it connects over TLS.
        #[arg(long)]
        tls_ca: Option<PathBuf>,
        /// Server name verified against the certificate when using TLS.
        #[arg(long, default_value = "localhost")]
        server_name: String,
        /// Emit the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Verify on-disk consistency (storage, catalog, indexes, planner stats,
    /// and any cluster Raft/snapshot state).
    Check {
        #[arg(long, default_value = ".local/auradb")]
        data_dir: PathBuf,
        /// Emit a structured JSON report. Exits non-zero if any check fails.
        #[arg(long)]
        json: bool,
    },
    /// Compact the storage log.
    Compact {
        #[arg(long, default_value = ".local/auradb")]
        data_dir: PathBuf,
    },
    /// Reclaim old MVCC versions no active transaction can observe.
    Gc {
        #[arg(long, default_value = ".local/auradb")]
        data_dir: PathBuf,
        /// Report what would be reclaimed without modifying any data.
        #[arg(long)]
        dry_run: bool,
        /// Emit the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Planner statistics (analyze / show).
    Stats {
        #[command(subcommand)]
        command: StatsCommand,
    },
    /// Export all schemas and records to a JSONL file.
    Dump {
        #[arg(long, default_value = ".local/auradb")]
        data_dir: PathBuf,
        /// Output file.
        #[arg(long, visible_alias = "output")]
        out: PathBuf,
    },
    /// Restore schemas and records from a JSONL dump.
    Restore {
        #[arg(long, default_value = ".local/auradb")]
        data_dir: PathBuf,
        /// Input dump file.
        #[arg(long, name = "in", visible_alias = "input")]
        input: PathBuf,
    },
    /// Backup utilities (`backup verify`).
    Backup {
        #[command(subcommand)]
        command: BackupCommand,
    },
    /// Run the local benchmark suite, or compare two baselines (`bench compare`).
    Bench {
        /// Optional subcommand (`compare`); omit to run the suite.
        #[command(subcommand)]
        command: Option<BenchCommand>,
        #[arg(long, default_value = ".local/auradb-bench")]
        data_dir: PathBuf,
        /// Number of records to insert.
        #[arg(long, default_value_t = 10_000)]
        records: usize,
        /// Emit the full report as JSON instead of a text summary.
        #[arg(long)]
        json: bool,
        /// Write the JSON report to this path (implies --json).
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Authentication helpers.
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    /// TLS certificate helpers.
    Cert {
        #[command(subcommand)]
        command: CertCommand,
    },
    /// Persisted index maintenance.
    Index {
        #[command(subcommand)]
        command: IndexCommand,
    },
    /// Search and ranking utilities (`search explain`).
    Search {
        #[command(subcommand)]
        command: SearchCommand,
    },
    /// Vector utilities (`vector eval`: ANN-preview recall/latency vs exact).
    Vector {
        #[command(subcommand)]
        command: VectorCommand,
    },
    /// Cluster (Raft) administration.
    Cluster {
        #[command(subcommand)]
        command: ClusterCommand,
    },
    /// Snapshot create / inspect / restore.
    Snapshot {
        #[command(subcommand)]
        command: SnapshotCommand,
    },
}

#[derive(Subcommand)]
enum SnapshotCommand {
    /// Capture a portable snapshot of a data directory to a file.
    Create {
        #[arg(long, default_value = ".local/auradb")]
        data_dir: PathBuf,
        /// Output snapshot file.
        #[arg(long, visible_alias = "out")]
        output: PathBuf,
    },
    /// Print a snapshot manifest and verify its integrity.
    Inspect {
        /// Input snapshot file.
        #[arg(long, name = "in", visible_alias = "input")]
        input: PathBuf,
    },
    /// Restore a snapshot file into a data directory.
    Restore {
        /// Input snapshot file.
        #[arg(long, name = "in", visible_alias = "input")]
        input: PathBuf,
        #[arg(long, default_value = ".local/auradb-restore")]
        data_dir: PathBuf,
        /// Overwrite a non-empty target directory.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum ClusterCommand {
    /// Create stable node and cluster identity if not already present.
    Init {
        #[arg(long, default_value = ".local/auradb")]
        data_dir: PathBuf,
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Show cluster status. With `--addr`, query a running server for live
    /// runtime diagnostics (role, leader, quorum, indices, per-peer state);
    /// otherwise show the offline metadata view for `--data-dir`.
    Status {
        #[arg(long, default_value = ".local/auradb")]
        data_dir: PathBuf,
        #[arg(long)]
        config: Option<PathBuf>,
        /// Query a running server's client port for live cluster diagnostics
        /// instead of reading on-disk metadata.
        #[arg(long)]
        addr: Option<String>,
        /// Authentication token, if the queried server requires auth.
        #[arg(long)]
        token: Option<String>,
        /// PEM CA bundle to enable and verify TLS when querying `--addr`.
        #[arg(long)]
        tls_ca: Option<PathBuf>,
        /// TLS server name to verify against when querying `--addr`.
        #[arg(long, default_value = "localhost")]
        server_name: String,
        /// Emit the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// List configured cluster peers.
    Peers {
        #[arg(long, default_value = ".local/auradb")]
        data_dir: PathBuf,
        #[arg(long)]
        config: Option<PathBuf>,
        /// Emit the peers as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Validate cluster configuration and metadata. With `--addr`, query a
    /// running server for live diagnostics (quorum, snapshot-needed followers,
    /// follower lag, quorum impact) instead of reading on-disk metadata.
    Doctor {
        #[arg(long, default_value = ".local/auradb")]
        data_dir: PathBuf,
        #[arg(long)]
        config: Option<PathBuf>,
        /// Query a running server's client port for live cluster diagnostics
        /// instead of validating on-disk config and metadata.
        #[arg(long)]
        addr: Option<String>,
        /// Authentication token, if the queried server requires auth.
        #[arg(long)]
        token: Option<String>,
        /// PEM CA bundle to enable and verify TLS when querying `--addr`.
        #[arg(long)]
        tls_ca: Option<PathBuf>,
        /// TLS server name to verify against when querying `--addr`.
        #[arg(long, default_value = "localhost")]
        server_name: String,
        /// Emit the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Bootstrap a durable single-node cluster and elect this node leader.
    Bootstrap {
        #[arg(long, default_value = ".local/auradb")]
        data_dir: PathBuf,
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Compact the durable Raft log up to the safely-applied prefix.
    CompactLog {
        #[arg(long, default_value = ".local/auradb")]
        data_dir: PathBuf,
        #[arg(long)]
        config: Option<PathBuf>,
        /// Report what would be compacted without modifying any data.
        #[arg(long)]
        dry_run: bool,
        /// Emit the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Plan (dry-run) a logical backup of a data dir: what it would include,
    /// exclude, where it can be restored, and which secrets are referenced.
    BackupPlan {
        #[arg(long, default_value = ".local/auradb")]
        data_dir: PathBuf,
        #[arg(long)]
        config: Option<PathBuf>,
        /// Emit the plan as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Plan (dry-run) a restore from a JSONL logical backup: what it would load
    /// and where it can be restored.
    RestorePlan {
        /// The JSONL logical backup to inspect.
        #[arg(long)]
        input: PathBuf,
        /// Emit the plan as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Report the current leader as seen by a running server.
    Leader {
        /// Address of a running server's client port.
        #[arg(long, default_value = "127.0.0.1:7171")]
        addr: String,
        /// Authentication token, if the server requires auth.
        #[arg(long)]
        token: Option<String>,
        /// PEM CA bundle to enable and verify TLS.
        #[arg(long)]
        tls_ca: Option<PathBuf>,
        /// TLS server name to verify against.
        #[arg(long, default_value = "localhost")]
        server_name: String,
        /// Emit the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Wait until a running server reports a recognized leader.
    WaitLeader {
        /// Address of a running server's client port.
        #[arg(long, default_value = "127.0.0.1:7171")]
        addr: String,
        /// Maximum seconds to wait.
        #[arg(long, default_value_t = 30)]
        timeout_secs: u64,
        #[arg(long)]
        token: Option<String>,
        #[arg(long)]
        tls_ca: Option<PathBuf>,
        #[arg(long, default_value = "localhost")]
        server_name: String,
        #[arg(long)]
        json: bool,
    },
    /// Wait until a server is reachable and reports ready.
    WaitReady {
        /// Address of a running server's client port.
        #[arg(long, default_value = "127.0.0.1:7171")]
        addr: String,
        /// Maximum seconds to wait.
        #[arg(long, default_value_t = 30)]
        timeout_secs: u64,
        #[arg(long)]
        token: Option<String>,
        #[arg(long)]
        tls_ca: Option<PathBuf>,
        #[arg(long, default_value = "localhost")]
        server_name: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum BenchCommand {
    /// Compare two benchmark baseline JSON files and report per-benchmark change.
    Compare {
        /// Baseline report (the reference, e.g. the previous release).
        #[arg(long)]
        baseline: PathBuf,
        /// Current report to compare against the baseline.
        #[arg(long)]
        current: PathBuf,
        /// Exit non-zero if any benchmark regresses by more than this percent.
        /// Omit to only warn (the default; safe for normal CI).
        #[arg(long)]
        fail_threshold_percent: Option<f64>,
    },
}

#[derive(Subcommand)]
enum StatsCommand {
    /// Recompute and persist planner statistics.
    Analyze {
        #[arg(long, default_value = ".local/auradb")]
        data_dir: PathBuf,
    },
    /// Show current planner statistics.
    Show {
        #[arg(long, default_value = ".local/auradb")]
        data_dir: PathBuf,
        /// Emit statistics as JSON.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Validate a configuration file, failing on invalid or unsafe settings.
    Validate {
        #[arg(long, default_value = "AuraDB.toml")]
        config: PathBuf,
        /// Validate structure only; do not check that referenced TLS files
        /// exist on disk. Use this to validate a deployment template whose
        /// certificates live on the target host.
        #[arg(long)]
        no_file_checks: bool,
    },
}

#[derive(Subcommand)]
enum IndexCommand {
    /// Validate persisted indexes and report how they loaded.
    Check {
        #[arg(long, default_value = ".local/auradb")]
        data_dir: PathBuf,
    },
    /// Rebuild every index from storage and persist fresh snapshots.
    Rebuild {
        #[arg(long, default_value = ".local/auradb")]
        data_dir: PathBuf,
    },
}

#[derive(Subcommand)]
enum SearchCommand {
    /// Explain a query (read from a JSON file as a FindQuery IR), reporting the
    /// chosen ranked-retrieval plan. With `--analyze`, execute it and attach
    /// measured metrics.
    Explain {
        #[arg(long, default_value = ".local/auradb")]
        data_dir: PathBuf,
        /// Path to a JSON file containing a FindQuery IR.
        #[arg(long, visible_alias = "in")]
        input: PathBuf,
        /// Execute the query and attach EXPLAIN ANALYZE metrics.
        #[arg(long)]
        analyze: bool,
    },
}

#[derive(Subcommand)]
enum VectorCommand {
    /// Evaluate approximate (HNSW preview) recall@k and latency against the exact
    /// baseline over a deterministic set of query vectors. The query file holds
    /// one JSON array of floats per line. Results are dataset/machine specific.
    Eval {
        #[arg(long, default_value = ".local/auradb")]
        data_dir: PathBuf,
        /// The collection to evaluate.
        #[arg(long)]
        collection: String,
        /// The vector field to evaluate.
        #[arg(long)]
        field: String,
        /// Path to a file with one JSON array of floats (a query vector) per line.
        #[arg(long)]
        queries: PathBuf,
        /// Neighbours requested per query.
        #[arg(long, default_value_t = 10)]
        k: usize,
        /// The distance metric (`cosine`, `euclidean`, `dot_product`).
        #[arg(long, default_value = "cosine")]
        metric: String,
        /// The HNSW `efSearch` beam width for the approximate preview.
        #[arg(long, default_value_t = 64)]
        ef_search: usize,
        /// Emit the report as JSON (the only output format).
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum BackupCommand {
    /// Validate a JSONL backup without importing it.
    Verify {
        /// The backup (JSONL dump) file.
        #[arg(long, name = "in", visible_alias = "input")]
        input: PathBuf,
        /// Emit a structured JSON report. Exits non-zero if the backup is invalid.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum AuthCommand {
    /// Hash a token with Argon2id for use as `auth.token_hash` in the config.
    HashToken {
        /// The token to hash. If omitted, it is read from the terminal without
        /// echoing.
        #[arg(long)]
        token: Option<String>,
    },
    /// Replace the static token in a config file with a new Argon2id hash.
    RotateToken {
        /// The config file to rewrite.
        #[arg(long, default_value = "AuraDB.toml")]
        config: PathBuf,
        /// The new token. If omitted, it is read from the terminal without
        /// echoing.
        #[arg(long)]
        token: Option<String>,
        /// Back up the previous config to `<config>.bak` before rewriting.
        #[arg(long)]
        backup: bool,
    },
}

#[derive(Subcommand)]
enum CertCommand {
    /// Generate self-signed development certificates (a CA and a server
    /// certificate signed by it). For local testing only.
    GenerateDev {
        /// Directory to write the CA and certificate/key into.
        #[arg(long, default_value = ".local/certs")]
        out_dir: PathBuf,
        /// Certificate Common Name and output file stem (e.g. `node1` writes
        /// `node1.crt` / `node1.key`). Omit for the classic `server.crt` /
        /// `server.key` with CN `localhost`.
        #[arg(long)]
        server_name: Option<String>,
        /// Subject Alternative Name to include (repeatable). When omitted, the
        /// SANs default to the server name plus `localhost` and `127.0.0.1`.
        #[arg(long = "san")]
        san: Vec<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Version => println!("{}", cmd_version()),
        Command::Init { data_dir, config } => {
            cmd_init(&data_dir, &config)?;
            println!(
                "initialized data dir {} and wrote config {}",
                data_dir.display(),
                config.display()
            );
        }
        Command::Server {
            config,
            data_dir,
            bind,
            port,
            allow_insecure_bind,
        } => {
            let cfg = build_config(config.as_deref(), data_dir, bind, port, allow_insecure_bind)?;
            cmd_server(cfg).await?;
        }
        Command::Compatibility => println!("{}", cmd_compatibility()),
        Command::Config { command } => match command {
            ConfigCommand::Validate {
                config,
                no_file_checks,
            } => println!("{}", cmd_config_validate(&config, no_file_checks)?),
        },
        Command::Doctor {
            data_dir,
            config,
            json,
        } => {
            let cfg = build_config(config.as_deref(), Some(data_dir.clone()), None, None, false)?;
            if json {
                println!("{}", cmd_doctor_json(&data_dir, &cfg)?);
            } else {
                print!("{}", cmd_doctor(&data_dir, &cfg)?);
            }
        }
        Command::Status {
            addr,
            token,
            tls_ca,
            server_name,
            json,
        } => {
            if json {
                println!(
                    "{}",
                    cmd_status_json(&addr, token, tls_ca, &server_name).await?
                );
            } else {
                println!("{}", cmd_status(&addr, token, tls_ca, &server_name).await?);
            }
        }
        Command::Check { data_dir, json } => {
            if json {
                let (report, ok) = cmd_check_json(&data_dir)?;
                println!("{report}");
                if !ok {
                    std::process::exit(1);
                }
            } else {
                println!("{}", cmd_check(&data_dir)?);
            }
        }
        Command::Compact { data_dir } => println!("{}", cmd_compact(&data_dir)?),
        Command::Gc {
            data_dir,
            dry_run,
            json,
        } => println!("{}", cmd_gc(&data_dir, dry_run, json)?),
        Command::Stats { command } => match command {
            StatsCommand::Analyze { data_dir } => println!("{}", cmd_stats_analyze(&data_dir)?),
            StatsCommand::Show { data_dir, json } => {
                println!("{}", cmd_stats_show(&data_dir, json)?)
            }
        },
        Command::Dump { data_dir, out } => {
            let lines = cmd_dump(&data_dir, &out)?;
            println!("wrote {lines} line(s) to {}", out.display());
        }
        Command::Restore { data_dir, input } => {
            let n = cmd_restore(&data_dir, &input)?;
            println!("restored {n} record(s)");
        }
        Command::Backup { command } => match command {
            BackupCommand::Verify { input, json } => {
                let (report, ok) = cmd_backup_verify(&input)?;
                if json {
                    println!("{report}");
                } else if ok {
                    println!("backup is valid");
                } else {
                    println!("backup is INVALID; run with --json for details");
                }
                if !ok {
                    std::process::exit(1);
                }
            }
        },
        Command::Bench {
            command,
            data_dir,
            records,
            json,
            output,
        } => match command {
            Some(BenchCommand::Compare {
                baseline,
                current,
                fail_threshold_percent,
            }) => {
                let (out, regressed) =
                    cmd_bench_compare(&baseline, &current, fail_threshold_percent)?;
                println!("{out}");
                if regressed {
                    std::process::exit(1);
                }
            }
            None => {
                if json || output.is_some() {
                    let commit = current_commit();
                    let out = cmd_bench_json(&data_dir, records, commit, output.as_deref())?;
                    if let Some(path) = output.as_deref() {
                        println!("wrote benchmark report to {}", path.display());
                    } else {
                        println!("{out}");
                    }
                } else {
                    println!("{}", cmd_bench(&data_dir, records)?);
                }
            }
        },
        Command::Auth { command } => match command {
            AuthCommand::HashToken { token } => println!("{}", cmd_auth_hash_token(token)?),
            AuthCommand::RotateToken {
                config,
                token,
                backup,
            } => println!("{}", cmd_auth_rotate_token(&config, token, backup)?),
        },
        Command::Cert { command } => match command {
            CertCommand::GenerateDev {
                out_dir,
                server_name,
                san,
            } => {
                println!("{}", cmd_cert_generate_dev(&out_dir, server_name, san)?)
            }
        },
        Command::Search { command } => match command {
            SearchCommand::Explain {
                data_dir,
                input,
                analyze,
            } => println!("{}", cmd_search_explain(&data_dir, &input, analyze)?),
        },
        Command::Vector { command } => match command {
            VectorCommand::Eval {
                data_dir,
                collection,
                field,
                queries,
                k,
                metric,
                ef_search,
                json: _,
            } => println!(
                "{}",
                cmd_vector_eval(
                    &data_dir,
                    &collection,
                    &field,
                    &queries,
                    k,
                    &metric,
                    ef_search
                )?
            ),
        },
        Command::Index { command } => match command {
            IndexCommand::Check { data_dir } => println!("{}", cmd_index_check(&data_dir)?),
            IndexCommand::Rebuild { data_dir } => println!("{}", cmd_index_rebuild(&data_dir)?),
        },
        Command::Cluster { command } => match command {
            ClusterCommand::Init { data_dir, config } => {
                let cfg =
                    build_config(config.as_deref(), Some(data_dir.clone()), None, None, false)?;
                print!("{}", cmd_cluster_init(&data_dir, &cfg)?);
            }
            ClusterCommand::Status {
                data_dir,
                config,
                addr,
                token,
                tls_ca,
                server_name,
                json,
            } => {
                if let Some(addr) = addr {
                    print!(
                        "{}",
                        cmd_cluster_status_live(&addr, token, tls_ca, &server_name, json).await?
                    );
                    if json {
                        println!();
                    }
                } else {
                    let cfg =
                        build_config(config.as_deref(), Some(data_dir.clone()), None, None, false)?;
                    print!("{}", cmd_cluster_status(&data_dir, &cfg, json)?);
                    if json {
                        println!();
                    }
                }
            }
            ClusterCommand::Peers {
                data_dir,
                config,
                json,
            } => {
                let cfg =
                    build_config(config.as_deref(), Some(data_dir.clone()), None, None, false)?;
                print!("{}", cmd_cluster_peers(&data_dir, &cfg, json)?);
                if json {
                    println!();
                }
            }
            ClusterCommand::Doctor {
                data_dir,
                config,
                addr,
                token,
                tls_ca,
                server_name,
                json,
            } => {
                if let Some(addr) = addr {
                    println!(
                        "{}",
                        cmd_cluster_doctor_live(&addr, token, tls_ca, &server_name, json).await?
                    );
                } else {
                    let cfg =
                        build_config(config.as_deref(), Some(data_dir.clone()), None, None, false)?;
                    print!("{}", cmd_cluster_doctor(&data_dir, &cfg, json)?);
                    if json {
                        println!();
                    }
                }
            }
            ClusterCommand::Bootstrap { data_dir, config } => {
                let cfg =
                    build_config(config.as_deref(), Some(data_dir.clone()), None, None, false)?;
                print!("{}", cmd_cluster_bootstrap(&data_dir, &cfg)?);
            }
            ClusterCommand::CompactLog {
                data_dir,
                config,
                dry_run,
                json,
            } => {
                let cfg =
                    build_config(config.as_deref(), Some(data_dir.clone()), None, None, false)?;
                print!(
                    "{}",
                    cmd_cluster_compact_log(&data_dir, &cfg, dry_run, json)?
                );
                if json {
                    println!();
                }
            }
            ClusterCommand::BackupPlan {
                data_dir,
                config,
                json,
            } => {
                let cfg =
                    build_config(config.as_deref(), Some(data_dir.clone()), None, None, false)?;
                print!("{}", cmd_cluster_backup_plan(&data_dir, &cfg, json)?);
                if json {
                    println!();
                }
            }
            ClusterCommand::RestorePlan { input, json } => {
                print!("{}", cmd_cluster_restore_plan(&input, json)?);
                if json {
                    println!();
                }
            }
            ClusterCommand::Leader {
                addr,
                token,
                tls_ca,
                server_name,
                json,
            } => {
                println!(
                    "{}",
                    cmd_cluster_leader(&addr, token, tls_ca, &server_name, json).await?
                );
            }
            ClusterCommand::WaitLeader {
                addr,
                timeout_secs,
                token,
                tls_ca,
                server_name,
                json,
            } => {
                println!(
                    "{}",
                    cmd_cluster_wait_leader(&addr, timeout_secs, token, tls_ca, &server_name, json)
                        .await?
                );
            }
            ClusterCommand::WaitReady {
                addr,
                timeout_secs,
                token,
                tls_ca,
                server_name,
                json,
            } => {
                println!(
                    "{}",
                    cmd_cluster_wait_ready(&addr, timeout_secs, token, tls_ca, &server_name, json)
                        .await?
                );
            }
        },
        Command::Snapshot { command } => match command {
            SnapshotCommand::Create { data_dir, output } => {
                println!("{}", cmd_snapshot_create(&data_dir, &output)?)
            }
            SnapshotCommand::Inspect { input } => {
                println!("{}", cmd_snapshot_inspect(&input)?)
            }
            SnapshotCommand::Restore {
                input,
                data_dir,
                force,
            } => println!("{}", cmd_snapshot_restore(&input, &data_dir, force)?),
        },
    }
    Ok(())
}

/// Best-effort short commit hash for benchmark provenance. Returns `None` when
/// git is unavailable or the directory is not a repository.
fn current_commit() -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let hash = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if hash.is_empty() {
        None
    } else {
        Some(hash)
    }
}
