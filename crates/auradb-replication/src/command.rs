//! The replicated command model.
//!
//! A [`ReplicatedCommand`] is the database-level meaning carried inside a Raft
//! [`Command`]'s opaque payload. The mapping is explicit and versioned: the
//! replication layer owns the payload format, while the Raft core only sees
//! framed bytes and a [`CommandKind`] tag. Keeping the two layers separate means
//! Raft never has to understand storage batches or schemas.

use serde::{Deserialize, Serialize};

use auradb_core::CollectionSchema;
use auradb_raft::{Command, CommandKind};
use auradb_storage::Batch;

use crate::error::{ReplicationError, Result};

/// The replication payload envelope version.
pub const ENVELOPE_VERSION: u16 = 1;

/// A schema change expressed as a replicated command.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SchemaCommand {
    /// Create or replace a collection schema.
    Create(Box<CollectionSchema>),
    /// Drop a collection.
    Drop(String),
}

/// A database mutation that can be ordered and replicated through Raft.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReplicatedCommand {
    /// A no-op (mirrors a Raft leader no-op; applies to nothing).
    Noop,
    /// A committed data-plane write batch.
    Write(Batch),
    /// A schema change.
    Schema(SchemaCommand),
}

impl ReplicatedCommand {
    /// The Raft command kind this command maps to.
    pub fn kind(&self) -> CommandKind {
        match self {
            ReplicatedCommand::Noop => CommandKind::Noop,
            ReplicatedCommand::Write(_) => CommandKind::Database,
            ReplicatedCommand::Schema(_) => CommandKind::Schema,
        }
    }

    /// Encode into a framed, versioned Raft [`Command`].
    pub fn encode(&self) -> Result<Command> {
        let envelope = Envelope {
            version: ENVELOPE_VERSION,
            command: self.clone(),
        };
        let payload =
            serde_json::to_vec(&envelope).map_err(|e| ReplicationError::Codec(e.to_string()))?;
        Ok(Command::new(self.kind(), payload))
    }

    /// Decode from a Raft [`Command`], rejecting unknown future envelopes.
    pub fn decode(command: &Command) -> Result<ReplicatedCommand> {
        if command.kind == CommandKind::Noop && command.payload.is_empty() {
            // A bare Raft no-op (e.g. a leader's term anchor) carries no payload.
            return Ok(ReplicatedCommand::Noop);
        }
        let envelope: Envelope = serde_json::from_slice(&command.payload)
            .map_err(|e| ReplicationError::Codec(e.to_string()))?;
        if envelope.version > ENVELOPE_VERSION {
            return Err(ReplicationError::UnsupportedVersion {
                found: envelope.version,
                supported: ENVELOPE_VERSION,
            });
        }
        Ok(envelope.command)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Envelope {
    version: u16,
    command: ReplicatedCommand,
}

#[cfg(test)]
mod tests {
    use super::*;
    use auradb_core::{CollectionId, Document, Record, RecordId, TxnId, Value};
    use auradb_storage::LogOp;

    fn sample_batch() -> Batch {
        let mut fields = Document::new();
        fields.insert("v".into(), Value::Int(1));
        Batch {
            txn_id: TxnId(1),
            ops: vec![LogOp::Put {
                commit_ts: 0,
                record: Record::new(RecordId::from_u128(1), CollectionId::new("C"), fields),
            }],
        }
    }

    #[test]
    fn write_command_roundtrips() {
        let cmd = ReplicatedCommand::Write(sample_batch());
        let encoded = cmd.encode().unwrap();
        assert_eq!(encoded.kind, CommandKind::Database);
        let back = ReplicatedCommand::decode(&encoded).unwrap();
        assert_eq!(cmd, back);
    }

    #[test]
    fn bare_noop_decodes() {
        let noop = Command::noop();
        assert_eq!(
            ReplicatedCommand::decode(&noop).unwrap(),
            ReplicatedCommand::Noop
        );
    }

    #[test]
    fn future_envelope_is_rejected() {
        // Hand-craft an envelope with a future version.
        let payload = serde_json::to_vec(&serde_json::json!({
            "version": ENVELOPE_VERSION + 1,
            "command": { "type": "noop" }
        }))
        .unwrap();
        let cmd = Command::new(CommandKind::Database, payload);
        assert!(matches!(
            ReplicatedCommand::decode(&cmd),
            Err(ReplicationError::UnsupportedVersion { .. })
        ));
    }
}
