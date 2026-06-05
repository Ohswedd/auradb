//! # auradb-raft
//!
//! A minimal, deterministic Raft log and consensus core for AuraDB.
//!
//! This crate provides the building blocks for replicated consensus:
//!
//! - a durable, checksummed [`log`] abstraction ([`LogEntry`], [`Command`],
//!   [`HardState`], the [`RaftStorage`] trait, an in-memory [`MemStorage`], and
//!   a file-backed [`FileStorage`]);
//! - a tick-driven [`RaftNode`] state machine (follower / candidate / leader,
//!   elections, `RequestVote`, `AppendEntries`, commit advancement) with a
//!   **logical clock** so behavior is reproducible and never timing-flaky;
//! - a deterministic in-process [`Sim`] harness for multi-node tests.
//!
//! ## Scope and honesty
//!
//! The consensus algorithm here is correct and tested for leader election, log
//! replication, log repair, and commit advancement. It does **not** yet
//! implement membership changes (joint consensus) or log snapshots/compaction
//! beyond the boundary in `auradb-replication`. Real network transport lives
//! outside this crate; AuraDB v0.4.0 drives this core in single-node mode in the
//! server and exercises multi-node consensus through the in-memory [`Sim`].

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod error;
mod log;
mod node;
mod sim;
mod storage;

pub use error::{RaftError, Result};
pub use log::{
    Command, CommandKind, HardState, LogEntry, LogIndex, MemStorage, RaftStorage, Term,
    COMMAND_VERSION,
};
pub use node::{single_node, Envelope, Message, RaftConfig, RaftMetrics, RaftNode};
pub use sim::Sim;
pub use storage::{CompactionOutcome, FileStorage};
