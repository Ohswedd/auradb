//! Frame opcodes (frame types).
//!
//! Opcodes identify the operation a frame represents. Request opcodes are sent
//! by clients; response opcodes are sent by the server. The numeric values are
//! part of the wire contract and must remain stable.

use auradb_core::{Error, Result};

/// A frame opcode. This enum is the concrete operation set implemented by the
/// single-node server and defines the protocol frame-type model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Opcode {
    // --- connection / liveness ---
    /// Client handshake: negotiate version, receive capabilities.
    Hello = 0x01,
    /// Liveness probe.
    Ping = 0x02,
    /// Health/readiness request.
    Health = 0x03,

    // --- schema ---
    /// Register (create) a collection schema.
    SchemaCreate = 0x10,
    /// Drop a collection schema.
    SchemaDrop = 0x11,
    /// Fetch one collection schema.
    SchemaGet = 0x12,
    /// List all collection schemas.
    SchemaList = 0x13,

    // --- data ---
    /// Read query (find / count / exists / vector nearest).
    Query = 0x20,
    /// Mutation (insert / bulk insert / update / delete / upsert).
    Mutate = 0x21,
    /// Fetch the next page from a server-side cursor.
    CursorFetch = 0x22,
    /// Close a server-side cursor.
    CursorClose = 0x23,
    /// Produce an EXPLAIN plan for a query.
    Explain = 0x24,
    /// Estimate the impact of a schema migration.
    MigrationEstimate = 0x25,

    // --- transactions ---
    /// Begin a transaction.
    TxnBegin = 0x30,
    /// Commit a transaction.
    TxnCommit = 0x31,
    /// Roll back a transaction.
    TxnRollback = 0x32,

    // --- responses ---
    /// Handshake acknowledgement carrying server capabilities.
    HelloAck = 0x81,
    /// Liveness reply.
    Pong = 0x82,
    /// Health/readiness reply.
    HealthResult = 0x83,
    /// Generic success carrying a JSON result payload.
    Ok = 0x90,
    /// A query result page (may carry a cursor id for continuation).
    QueryResult = 0x91,
    /// Structured error frame.
    Error = 0xFF,
}

impl Opcode {
    /// Decode an opcode from its byte representation.
    pub fn from_u8(v: u8) -> Result<Opcode> {
        Ok(match v {
            0x01 => Opcode::Hello,
            0x02 => Opcode::Ping,
            0x03 => Opcode::Health,
            0x10 => Opcode::SchemaCreate,
            0x11 => Opcode::SchemaDrop,
            0x12 => Opcode::SchemaGet,
            0x13 => Opcode::SchemaList,
            0x20 => Opcode::Query,
            0x21 => Opcode::Mutate,
            0x22 => Opcode::CursorFetch,
            0x23 => Opcode::CursorClose,
            0x24 => Opcode::Explain,
            0x25 => Opcode::MigrationEstimate,
            0x30 => Opcode::TxnBegin,
            0x31 => Opcode::TxnCommit,
            0x32 => Opcode::TxnRollback,
            0x81 => Opcode::HelloAck,
            0x82 => Opcode::Pong,
            0x83 => Opcode::HealthResult,
            0x90 => Opcode::Ok,
            0x91 => Opcode::QueryResult,
            0xFF => Opcode::Error,
            other => return Err(Error::Protocol(format!("unknown opcode: 0x{other:02x}"))),
        })
    }

    /// The byte representation of this opcode.
    pub fn as_u8(self) -> u8 {
        self as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opcode_roundtrips() {
        for op in [
            Opcode::Hello,
            Opcode::Query,
            Opcode::Mutate,
            Opcode::CursorFetch,
            Opcode::Error,
            Opcode::TxnCommit,
        ] {
            assert_eq!(Opcode::from_u8(op.as_u8()).unwrap(), op);
        }
    }

    #[test]
    fn unknown_opcode_rejected() {
        assert!(Opcode::from_u8(0x77).is_err());
    }
}
