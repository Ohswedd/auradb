//! Applying committed replicated commands to the engine.
//!
//! Apply is the bridge from the ordered Raft log to engine state. It is the
//! same on a leader recovering after a crash and on a follower receiving the
//! leader's committed entries: decode the command, then route it to the engine's
//! idempotent apply path. Idempotency lives in the engine
//! ([`Engine::apply_replicated_batch`] is a no-op below the commit watermark), so
//! replaying a prefix of the log is always safe.

use auradb::Engine;

use crate::command::{ReplicatedCommand, SchemaCommand};
use crate::error::Result;

/// Apply one committed command, identified by its Raft `log_index`.
///
/// The `log_index` doubles as the MVCC commit timestamp for data writes, which
/// is why it must be the entry's real index in the log.
pub fn apply_command(engine: &Engine, command: &ReplicatedCommand, log_index: u64) -> Result<()> {
    match command {
        ReplicatedCommand::Noop => Ok(()),
        ReplicatedCommand::Write(batch) => {
            engine.apply_replicated_batch(batch.clone(), log_index)?;
            Ok(())
        }
        ReplicatedCommand::Schema(SchemaCommand::Create(schema)) => {
            engine.create_schema((**schema).clone())?;
            Ok(())
        }
        ReplicatedCommand::Schema(SchemaCommand::Drop(name)) => {
            engine.drop_schema(name)?;
            Ok(())
        }
    }
}
