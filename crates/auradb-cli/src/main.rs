//! The `auradb` command-line interface.
#![forbid(unsafe_code)]

use std::path::PathBuf;

use anyhow::Result;
use auradb_cli::{
    build_config, cmd_auth_hash_token, cmd_auth_rotate_token, cmd_bench, cmd_bench_json,
    cmd_cert_generate_dev, cmd_check, cmd_compact, cmd_compatibility, cmd_config_validate,
    cmd_doctor, cmd_doctor_json, cmd_dump, cmd_index_check, cmd_index_rebuild, cmd_init,
    cmd_restore, cmd_server, cmd_status, cmd_status_json, cmd_version,
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
    /// Verify on-disk index consistency.
    Check {
        #[arg(long, default_value = ".local/auradb")]
        data_dir: PathBuf,
    },
    /// Compact the storage log.
    Compact {
        #[arg(long, default_value = ".local/auradb")]
        data_dir: PathBuf,
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
    /// Run the local benchmark suite.
    Bench {
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
        /// Directory to write ca.crt, ca.key, server.crt, and server.key into.
        #[arg(long, default_value = ".local/certs")]
        out_dir: PathBuf,
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
        Command::Check { data_dir } => println!("{}", cmd_check(&data_dir)?),
        Command::Compact { data_dir } => println!("{}", cmd_compact(&data_dir)?),
        Command::Dump { data_dir, out } => {
            let lines = cmd_dump(&data_dir, &out)?;
            println!("wrote {lines} line(s) to {}", out.display());
        }
        Command::Restore { data_dir, input } => {
            let n = cmd_restore(&data_dir, &input)?;
            println!("restored {n} record(s)");
        }
        Command::Bench {
            data_dir,
            records,
            json,
            output,
        } => {
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
        Command::Auth { command } => match command {
            AuthCommand::HashToken { token } => println!("{}", cmd_auth_hash_token(token)?),
            AuthCommand::RotateToken {
                config,
                token,
                backup,
            } => println!("{}", cmd_auth_rotate_token(&config, token, backup)?),
        },
        Command::Cert { command } => match command {
            CertCommand::GenerateDev { out_dir } => {
                println!("{}", cmd_cert_generate_dev(&out_dir)?)
            }
        },
        Command::Index { command } => match command {
            IndexCommand::Check { data_dir } => println!("{}", cmd_index_check(&data_dir)?),
            IndexCommand::Rebuild { data_dir } => println!("{}", cmd_index_rebuild(&data_dir)?),
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
