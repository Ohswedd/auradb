//! On-disk log format.
//!
//! A segment file is a sequence of *batch frames*. Each frame is:
//!
//! ```text
//! [payload length: u64 BE][payload crc32: u32 BE][payload bytes]
//! ```
//!
//! The payload is the JSON encoding of a [`Batch`]. Because each batch is a
//! single length-prefixed, checksummed unit, a batch is atomic: a partially
//! written (torn) trailing batch is detected and truncated on recovery, and a
//! checksum mismatch on a fully present batch is reported as corruption.

use auradb_core::{CollectionId, Error, Record, RecordId, Result, TxnId};
use serde::{Deserialize, Serialize};

/// A single mutation recorded in the log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum LogOp {
    /// Insert or replace a record.
    Put {
        /// The record being written.
        record: Record,
    },
    /// Delete a record (tombstone).
    Delete {
        /// The collection the record lives in.
        collection: CollectionId,
        /// The record id to remove.
        id: RecordId,
    },
}

/// An atomic group of mutations produced by one committed transaction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Batch {
    /// The transaction that produced this batch.
    pub txn_id: TxnId,
    /// The ordered mutations.
    pub ops: Vec<LogOp>,
}

impl Batch {
    /// Encode this batch into a self-describing, checksummed frame.
    pub fn encode(&self) -> Vec<u8> {
        let payload = serde_json::to_vec(self).expect("batch always serializes");
        let crc = crc32fast::hash(&payload);
        let mut out = Vec::with_capacity(12 + payload.len());
        out.extend_from_slice(&(payload.len() as u64).to_be_bytes());
        out.extend_from_slice(&crc.to_be_bytes());
        out.extend_from_slice(&payload);
        out
    }
}

/// The outcome of parsing batches from a segment buffer.
pub struct ParsedSegment {
    /// Fully decoded batches in order.
    pub batches: Vec<Batch>,
    /// The number of valid bytes consumed (the file should be truncated here to
    /// drop any torn trailing batch).
    pub valid_len: usize,
    /// Whether a torn trailing batch was detected and dropped.
    pub truncated: bool,
}

/// Parse all batches from a segment buffer.
///
/// Stops at a torn trailing batch (length runs past the end of the buffer),
/// recording `valid_len` so the caller can truncate. A checksum mismatch or a
/// malformed payload in a fully present batch is reported as corruption - the
/// engine fails closed rather than silently dropping committed data.
pub fn parse_segment(buf: &[u8]) -> Result<ParsedSegment> {
    let mut batches = Vec::new();
    let mut offset = 0usize;
    loop {
        if offset == buf.len() {
            return Ok(ParsedSegment {
                batches,
                valid_len: offset,
                truncated: false,
            });
        }
        // Need at least the 12-byte frame header.
        if offset + 12 > buf.len() {
            return Ok(ParsedSegment {
                batches,
                valid_len: offset,
                truncated: true,
            });
        }
        let len = u64::from_be_bytes(buf[offset..offset + 8].try_into().unwrap()) as usize;
        let crc = u32::from_be_bytes(buf[offset + 8..offset + 12].try_into().unwrap());
        let body_start = offset + 12;
        let body_end = match body_start.checked_add(len) {
            Some(e) => e,
            None => return Err(Error::Corruption("batch length overflow".into())),
        };
        if body_end > buf.len() {
            // Torn trailing batch: not fully written.
            return Ok(ParsedSegment {
                batches,
                valid_len: offset,
                truncated: true,
            });
        }
        let payload = &buf[body_start..body_end];
        if crc32fast::hash(payload) != crc {
            return Err(Error::Corruption(format!(
                "batch checksum mismatch at offset {offset}"
            )));
        }
        let batch: Batch = serde_json::from_slice(payload)
            .map_err(|e| Error::Corruption(format!("malformed batch at offset {offset}: {e}")))?;
        batches.push(batch);
        offset = body_end;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auradb_core::{Document, Value};

    fn rec(id: u128) -> Record {
        let mut fields = Document::new();
        fields.insert("v".into(), Value::Int(id as i64));
        Record::new(RecordId::from_u128(id), CollectionId::new("C"), fields)
    }

    fn batch(id: u128) -> Batch {
        Batch {
            txn_id: TxnId(id as u64),
            ops: vec![LogOp::Put { record: rec(id) }],
        }
    }

    #[test]
    fn roundtrip_batches() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&batch(1).encode());
        buf.extend_from_slice(&batch(2).encode());
        let parsed = parse_segment(&buf).unwrap();
        assert_eq!(parsed.batches.len(), 2);
        assert!(!parsed.truncated);
        assert_eq!(parsed.valid_len, buf.len());
    }

    #[test]
    fn torn_tail_is_truncated() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&batch(1).encode());
        let good_len = buf.len();
        buf.extend_from_slice(&batch(2).encode());
        buf.truncate(good_len + 6); // cut into the middle of batch 2
        let parsed = parse_segment(&buf).unwrap();
        assert_eq!(parsed.batches.len(), 1);
        assert!(parsed.truncated);
        assert_eq!(parsed.valid_len, good_len);
    }

    #[test]
    fn corrupted_full_batch_errors() {
        let mut buf = batch(1).encode();
        let n = buf.len();
        buf[n - 1] ^= 0xff; // flip a payload byte
        assert!(matches!(parse_segment(&buf), Err(Error::Corruption(_))));
    }

    #[test]
    fn empty_buffer_parses_clean() {
        let parsed = parse_segment(&[]).unwrap();
        assert!(parsed.batches.is_empty());
        assert!(!parsed.truncated);
    }
}
