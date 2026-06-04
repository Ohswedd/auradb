//! The `auradb` command-line interface.
#![forbid(unsafe_code)]

use std::path::PathBuf;

use anyhow::Result;
use auradb_cli::{
    build_config, cmd_bench, cmd_check, cmd_compact, cmd_doctor, cmd_dump, cmd_init, cmd_restore,
    cmd_server, cmd_status, cmd_version,
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
    },
    /// Validate config and data directory.
    Doctor {
        #[arg(long, default_value = ".local/auradb")]
        data_dir: PathBuf,
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Ping a running server and report health.
    Status {
        /// Server address (`host:port`).
        #[arg(long, default_value = "127.0.0.1:7171")]
        addr: String,
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
        #[arg(long)]
        out: PathBuf,
    },
    /// Restore schemas and records from a JSONL dump.
    Restore {
        #[arg(long, default_value = ".local/auradb")]
        data_dir: PathBuf,
        /// Input dump file.
        #[arg(long, name = "in")]
        input: PathBuf,
    },
    /// Run a local insert/read/vector benchmark.
    Bench {
        #[arg(long, default_value = ".local/auradb-bench")]
        data_dir: PathBuf,
        /// Number of records to insert.
        #[arg(long, default_value_t = 10_000)]
        records: usize,
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
        } => {
            let cfg = build_config(config.as_deref(), data_dir, bind, port)?;
            cmd_server(cfg).await?;
        }
        Command::Doctor { data_dir, config } => {
            let cfg = build_config(config.as_deref(), Some(data_dir.clone()), None, None)?;
            print!("{}", cmd_doctor(&data_dir, &cfg)?);
        }
        Command::Status { addr } => println!("{}", cmd_status(&addr).await?),
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
        Command::Bench { data_dir, records } => println!("{}", cmd_bench(&data_dir, records)?),
    }
    Ok(())
}
