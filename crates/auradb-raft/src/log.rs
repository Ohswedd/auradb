//! The Raft log abstraction: entries, commands, hard state, and the storage
//! trait every backend implements.
//!
//! The log is the heart of Raft durability. An entry is identified by a
//! `(term, index)` pair; the [`RaftStorage`] trait defines the small set of
//! operations the consensus core needs — append, read a range, truncate a
//! conflicting suffix, and persist the hard state (current term, vote, and
//! commit index). Two backends implement it: an in-memory [`MemStorage`] for
//! deterministic tests, and a durable file backend (see [`crate::FileStorage`]).

use serde::{Deserialize, Serialize};

use auradb_cluster::NodeId;

use crate::error::{RaftError, Result};

/// A Raft term: a logical clock that increases on every election.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Term(pub u64);

impl Term {
    /// The zero term, used as a sentinel before the first entry.
    pub const ZERO: Term = Term(0);

    /// The raw value.
    pub const fn get(self) -> u64 {
        self.0
    }

    /// The next term.
    pub const fn next(self) -> Term {
        Term(self.0 + 1)
    }
}

impl std::fmt::Display for Term {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A 1-based Raft log index. Index `0` means "before the first entry".
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct LogIndex(pub u64);

impl LogIndex {
    /// The sentinel "before first entry" index.
    pub const ZERO: LogIndex = LogIndex(0);

    /// The raw value.
    pub const fn get(self) -> u64 {
        self.0
    }

    /// The next index.
    pub const fn next(self) -> LogIndex {
        LogIndex(self.0 + 1)
    }

    /// The previous index, saturating at zero.
    pub const fn prev(self) -> LogIndex {
        LogIndex(self.0.saturating_sub(1))
    }
}

impl std::fmt::Display for LogIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The category of a replicated command.
///
/// The Raft core is database-agnostic: it never interprets a command's bytes.
/// The kind lets the apply layer (and diagnostics) route a payload without
/// decoding it, and it keeps the log self-describing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandKind {
    /// A leader no-op, used to anchor a new term's commit point.
    Noop,
    /// A data-plane mutation (a committed write batch).
    Database,
    /// A schema change.
    Schema,
    /// A cluster-metadata change.
    Metadata,
    /// A membership / configuration change.
    Config,
}

/// A framed, versioned replicated command.
///
/// The Raft core treats [`Command::payload`] as opaque bytes. The replication
/// layer owns the payload's meaning and its own internal versioning; the
/// `version` field here frames the envelope so a future format can be detected
/// and rejected rather than misread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Command {
    /// Envelope version. Bumped if the framing changes.
    pub version: u16,
    /// What kind of command this is.
    pub kind: CommandKind,
    /// The opaque, framed payload owned by the replication layer.
    #[serde(with = "base64_bytes")]
    pub payload: Vec<u8>,
}

/// The current command envelope version.
pub const COMMAND_VERSION: u16 = 1;

impl Command {
    /// Construct a command with the current envelope version.
    pub fn new(kind: CommandKind, payload: Vec<u8>) -> Command {
        Command {
            version: COMMAND_VERSION,
            kind,
            payload,
        }
    }

    /// A leader no-op command.
    pub fn noop() -> Command {
        Command::new(CommandKind::Noop, Vec::new())
    }
}

/// One entry in the Raft log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    /// The term in which the entry was created.
    pub term: Term,
    /// The entry's position in the log.
    pub index: LogIndex,
    /// The replicated command.
    pub command: Command,
}

/// The durable, crash-critical Raft state that must survive restarts.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HardState {
    /// The latest term this node has seen.
    pub current_term: Term,
    /// The candidate this node voted for in `current_term`, if any.
    pub voted_for: Option<NodeId>,
    /// The highest log index known to be committed.
    pub commit_index: LogIndex,
}

/// The storage interface the Raft core depends on.
///
/// Implementations must enforce two log invariants on [`append`](RaftStorage::append):
/// entries are contiguous (no index gaps) and terms never regress. The core
/// truncates a conflicting suffix before appending, so appends always extend the
/// tail.
pub trait RaftStorage {
    /// Append contiguous entries to the tail of the log.
    fn append(&mut self, entries: &[LogEntry]) -> Result<()>;

    /// Remove all entries at and after `index`.
    fn truncate_from(&mut self, index: LogIndex) -> Result<()>;

    /// The term of the entry at `index`, or [`Term::ZERO`] for index `0`.
    fn term_at(&self, index: LogIndex) -> Option<Term>;

    /// The entry at `index`, if present.
    fn entry_at(&self, index: LogIndex) -> Option<LogEntry>;

    /// All entries from `index` to the end of the log, inclusive.
    fn entries_from(&self, index: LogIndex) -> Vec<LogEntry>;

    /// The index of the last entry (0 if empty).
    fn last_index(&self) -> LogIndex;

    /// The term of the last entry (0 if empty).
    fn last_term(&self) -> Term;

    /// Persist the hard state durably.
    fn save_hard_state(&mut self, hs: &HardState) -> Result<()>;

    /// The persisted hard state.
    fn hard_state(&self) -> HardState;
}

/// Validate that `entries` form a contiguous, non-term-regressing suffix
/// extending a log whose current tail is `(last_index, last_term)`.
pub(crate) fn validate_suffix(
    entries: &[LogEntry],
    last_index: LogIndex,
    last_term: Term,
) -> Result<()> {
    let mut expect_index = last_index.next();
    let mut prev_term = last_term;
    for entry in entries {
        if entry.index != expect_index {
            return Err(RaftError::InvalidAppend(format!(
                "expected index {expect_index}, got {} (log gap)",
                entry.index
            )));
        }
        if entry.term < prev_term {
            return Err(RaftError::InvalidAppend(format!(
                "term regression at index {}: {} < {prev_term}",
                entry.index, entry.term
            )));
        }
        expect_index = expect_index.next();
        prev_term = entry.term;
    }
    Ok(())
}

/// An in-memory [`RaftStorage`] for tests and the single-node fast path.
#[derive(Debug, Default, Clone)]
pub struct MemStorage {
    entries: Vec<LogEntry>,
    hard_state: HardState,
}

impl MemStorage {
    /// A fresh, empty in-memory log.
    pub fn new() -> MemStorage {
        MemStorage::default()
    }
}

impl RaftStorage for MemStorage {
    fn append(&mut self, entries: &[LogEntry]) -> Result<()> {
        validate_suffix(entries, self.last_index(), self.last_term())?;
        self.entries.extend_from_slice(entries);
        Ok(())
    }

    fn truncate_from(&mut self, index: LogIndex) -> Result<()> {
        let keep = index.get().saturating_sub(1) as usize;
        if keep < self.entries.len() {
            self.entries.truncate(keep);
        }
        Ok(())
    }

    fn term_at(&self, index: LogIndex) -> Option<Term> {
        if index == LogIndex::ZERO {
            return Some(Term::ZERO);
        }
        self.entries.get((index.get() - 1) as usize).map(|e| e.term)
    }

    fn entry_at(&self, index: LogIndex) -> Option<LogEntry> {
        if index == LogIndex::ZERO {
            return None;
        }
        self.entries.get((index.get() - 1) as usize).cloned()
    }

    fn entries_from(&self, index: LogIndex) -> Vec<LogEntry> {
        if index == LogIndex::ZERO {
            return self.entries.clone();
        }
        let start = (index.get() - 1) as usize;
        self.entries
            .get(start..)
            .map(|s| s.to_vec())
            .unwrap_or_default()
    }

    fn last_index(&self) -> LogIndex {
        LogIndex(self.entries.len() as u64)
    }

    fn last_term(&self) -> Term {
        self.entries.last().map(|e| e.term).unwrap_or(Term::ZERO)
    }

    fn save_hard_state(&mut self, hs: &HardState) -> Result<()> {
        self.hard_state = hs.clone();
        Ok(())
    }

    fn hard_state(&self) -> HardState {
        self.hard_state.clone()
    }
}

mod base64_bytes {
    //! Compact, dependency-free base64 (standard alphabet) for command payloads
    //! so the JSON log stays human-readable and self-describing.
    use serde::{Deserialize, Deserializer, Serializer};

    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        decode(&s).map_err(serde::de::Error::custom)
    }

    pub fn encode(bytes: &[u8]) -> String {
        let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for chunk in bytes.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | (b[2] as u32);
            out.push(ALPHABET[((n >> 18) & 63) as usize] as char);
            out.push(ALPHABET[((n >> 12) & 63) as usize] as char);
            out.push(if chunk.len() > 1 {
                ALPHABET[((n >> 6) & 63) as usize] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 {
                ALPHABET[(n & 63) as usize] as char
            } else {
                '='
            });
        }
        out
    }

    pub fn decode(s: &str) -> Result<Vec<u8>, String> {
        fn val(c: u8) -> Result<u32, String> {
            match c {
                b'A'..=b'Z' => Ok((c - b'A') as u32),
                b'a'..=b'z' => Ok((c - b'a' + 26) as u32),
                b'0'..=b'9' => Ok((c - b'0' + 52) as u32),
                b'+' => Ok(62),
                b'/' => Ok(63),
                _ => Err(format!("invalid base64 byte {c:#x}")),
            }
        }
        let s = s.trim().as_bytes();
        if s.len() % 4 != 0 {
            return Err("base64 length must be a multiple of 4".into());
        }
        let mut out = Vec::with_capacity(s.len() / 4 * 3);
        for chunk in s.chunks(4) {
            let pad = chunk.iter().filter(|&&c| c == b'=').count();
            let mut n = 0u32;
            for (i, &c) in chunk.iter().enumerate() {
                let v = if c == b'=' { 0 } else { val(c)? };
                n |= v << (18 - 6 * i);
            }
            out.push((n >> 16) as u8);
            if pad < 2 {
                out.push((n >> 8) as u8);
            }
            if pad < 1 {
                out.push(n as u8);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(term: u64, index: u64) -> LogEntry {
        LogEntry {
            term: Term(term),
            index: LogIndex(index),
            command: Command::noop(),
        }
    }

    #[test]
    fn append_and_read_back() {
        let mut s = MemStorage::new();
        s.append(&[entry(1, 1), entry(1, 2)]).unwrap();
        assert_eq!(s.last_index(), LogIndex(2));
        assert_eq!(s.last_term(), Term(1));
        assert_eq!(s.term_at(LogIndex(1)), Some(Term(1)));
        assert_eq!(s.term_at(LogIndex(0)), Some(Term::ZERO));
        assert_eq!(s.entries_from(LogIndex(2)).len(), 1);
    }

    #[test]
    fn append_rejects_gap() {
        let mut s = MemStorage::new();
        assert!(matches!(
            s.append(&[entry(1, 2)]),
            Err(RaftError::InvalidAppend(_))
        ));
    }

    #[test]
    fn append_rejects_term_regression() {
        let mut s = MemStorage::new();
        s.append(&[entry(2, 1)]).unwrap();
        assert!(matches!(
            s.append(&[entry(1, 2)]),
            Err(RaftError::InvalidAppend(_))
        ));
    }

    #[test]
    fn truncate_suffix_drops_tail() {
        let mut s = MemStorage::new();
        s.append(&[entry(1, 1), entry(1, 2), entry(1, 3)]).unwrap();
        s.truncate_from(LogIndex(2)).unwrap();
        assert_eq!(s.last_index(), LogIndex(1));
        s.append(&[entry(2, 2)]).unwrap();
        assert_eq!(s.term_at(LogIndex(2)), Some(Term(2)));
    }

    #[test]
    fn base64_roundtrips() {
        for case in [&b""[..], b"a", b"ab", b"abc", b"hello, raft\x00\xff"] {
            let enc = base64_bytes::encode(case);
            let dec = base64_bytes::decode(&enc).unwrap();
            assert_eq!(dec, case);
        }
    }

    #[test]
    fn command_roundtrips_through_json() {
        let cmd = Command::new(CommandKind::Database, vec![1, 2, 3, 250]);
        let json = serde_json::to_string(&cmd).unwrap();
        let back: Command = serde_json::from_str(&json).unwrap();
        assert_eq!(cmd, back);
    }
}
