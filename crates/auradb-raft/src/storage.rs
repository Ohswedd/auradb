//! A durable file-backed [`RaftStorage`].
//!
//! Layout (under the directory the caller chooses, e.g. `<data_dir>/cluster/`):
//!
//! ```text
//! raft-log.bin      # append-only, framed, checksummed log entries
//! raft-state.json   # the hard state (current term, vote, commit index)
//! ```
//!
//! Each log frame is `[len: u64 BE][crc32: u32 BE][json bytes]`, so a torn
//! trailing frame (a crash mid-append) is detected and dropped on open, while a
//! checksum mismatch on a fully present frame fails closed as corruption — the
//! same discipline the storage engine uses for its segments.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::error::{RaftError, Result};
use crate::log::{validate_suffix, HardState, LogEntry, LogIndex, RaftStorage, Term};

const LOG_FILE: &str = "raft-log.bin";
const STATE_FILE: &str = "raft-state.json";

/// A durable Raft log + hard state backed by files in a directory.
#[derive(Debug)]
pub struct FileStorage {
    dir: PathBuf,
    log_path: PathBuf,
    state_path: PathBuf,
    file: File,
    entries: Vec<LogEntry>,
    hard_state: HardState,
}

impl FileStorage {
    /// Open (creating if absent) a durable Raft store in `dir`.
    ///
    /// Replays the log, validating checksums and the contiguity/term invariants,
    /// and loads the hard state. A torn trailing frame is truncated; any other
    /// integrity failure is reported as [`RaftError::Corruption`].
    pub fn open(dir: impl AsRef<Path>) -> Result<FileStorage> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir).map_err(|source| RaftError::Io {
            path: dir.clone(),
            source,
        })?;
        let log_path = dir.join(LOG_FILE);
        let state_path = dir.join(STATE_FILE);

        let (entries, valid_len) = Self::replay_log(&log_path)?;

        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&log_path)
            .map_err(|source| RaftError::Io {
                path: log_path.clone(),
                source,
            })?;
        // Drop any torn trailing frame so future appends extend valid data.
        let current_len = file
            .metadata()
            .map_err(|source| RaftError::Io {
                path: log_path.clone(),
                source,
            })?
            .len();
        if (valid_len as u64) < current_len {
            file.set_len(valid_len as u64)
                .map_err(|source| RaftError::Io {
                    path: log_path.clone(),
                    source,
                })?;
        }

        let hard_state = Self::load_state(&state_path)?;

        Ok(FileStorage {
            dir,
            log_path,
            state_path,
            file,
            entries,
            hard_state,
        })
    }

    /// The directory backing this store.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn replay_log(path: &Path) -> Result<(Vec<LogEntry>, usize)> {
        let mut buf = Vec::new();
        match File::open(path) {
            Ok(mut f) => {
                f.read_to_end(&mut buf).map_err(|source| RaftError::Io {
                    path: path.to_path_buf(),
                    source,
                })?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((Vec::new(), 0)),
            Err(source) => {
                return Err(RaftError::Io {
                    path: path.to_path_buf(),
                    source,
                })
            }
        }

        let mut entries: Vec<LogEntry> = Vec::new();
        let mut offset = 0usize;
        let mut last_index = LogIndex::ZERO;
        let mut last_term = Term::ZERO;
        while offset < buf.len() {
            if offset + 12 > buf.len() {
                break; // torn header
            }
            let len = u64::from_be_bytes(buf[offset..offset + 8].try_into().unwrap()) as usize;
            let crc = u32::from_be_bytes(buf[offset + 8..offset + 12].try_into().unwrap());
            let body_start = offset + 12;
            let body_end = match body_start.checked_add(len) {
                Some(e) => e,
                None => return Err(RaftError::Corruption("frame length overflow".into())),
            };
            if body_end > buf.len() {
                break; // torn trailing frame
            }
            let payload = &buf[body_start..body_end];
            if crc32fast::hash(payload) != crc {
                return Err(RaftError::Corruption(format!(
                    "raft log frame checksum mismatch at offset {offset}"
                )));
            }
            let entry: LogEntry = serde_json::from_slice(payload).map_err(|e| {
                RaftError::Corruption(format!("malformed raft entry at offset {offset}: {e}"))
            })?;
            validate_suffix(std::slice::from_ref(&entry), last_index, last_term)
                .map_err(|e| RaftError::Corruption(e.to_string()))?;
            last_index = entry.index;
            last_term = entry.term;
            entries.push(entry);
            offset = body_end;
        }
        Ok((entries, offset))
    }

    fn load_state(path: &Path) -> Result<HardState> {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| RaftError::Corruption(format!("malformed raft hard state: {e}"))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HardState::default()),
            Err(source) => Err(RaftError::Io {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    fn encode_frame(entry: &LogEntry) -> Result<Vec<u8>> {
        let payload = serde_json::to_vec(entry)
            .map_err(|e| RaftError::Codec(format!("encoding raft entry: {e}")))?;
        let crc = crc32fast::hash(&payload);
        let mut out = Vec::with_capacity(12 + payload.len());
        out.extend_from_slice(&(payload.len() as u64).to_be_bytes());
        out.extend_from_slice(&crc.to_be_bytes());
        out.extend_from_slice(&payload);
        Ok(out)
    }

    fn rewrite_log(&mut self) -> Result<()> {
        let mut bytes = Vec::new();
        for entry in &self.entries {
            bytes.extend_from_slice(&Self::encode_frame(entry)?);
        }
        let tmp = self.log_path.with_extension("bin.tmp");
        {
            let mut f = File::create(&tmp).map_err(|source| RaftError::Io {
                path: tmp.clone(),
                source,
            })?;
            f.write_all(&bytes).map_err(|source| RaftError::Io {
                path: tmp.clone(),
                source,
            })?;
            f.sync_all().map_err(|source| RaftError::Io {
                path: tmp.clone(),
                source,
            })?;
        }
        std::fs::rename(&tmp, &self.log_path).map_err(|source| RaftError::Io {
            path: self.log_path.clone(),
            source,
        })?;
        // Reopen the append handle on the freshly written file.
        self.file = OpenOptions::new()
            .read(true)
            .append(true)
            .open(&self.log_path)
            .map_err(|source| RaftError::Io {
                path: self.log_path.clone(),
                source,
            })?;
        Ok(())
    }
}

impl RaftStorage for FileStorage {
    fn append(&mut self, entries: &[LogEntry]) -> Result<()> {
        validate_suffix(entries, self.last_index(), self.last_term())?;
        if entries.is_empty() {
            return Ok(());
        }
        let mut bytes = Vec::new();
        for entry in entries {
            bytes.extend_from_slice(&Self::encode_frame(entry)?);
        }
        self.file
            .write_all(&bytes)
            .map_err(|source| RaftError::Io {
                path: self.log_path.clone(),
                source,
            })?;
        self.file.sync_all().map_err(|source| RaftError::Io {
            path: self.log_path.clone(),
            source,
        })?;
        self.entries.extend_from_slice(entries);
        Ok(())
    }

    fn truncate_from(&mut self, index: LogIndex) -> Result<()> {
        let keep = index.get().saturating_sub(1) as usize;
        if keep < self.entries.len() {
            self.entries.truncate(keep);
            self.rewrite_log()?;
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
        let text = serde_json::to_string_pretty(hs)
            .map_err(|e| RaftError::Codec(format!("encoding hard state: {e}")))?;
        let tmp = self.state_path.with_extension("json.tmp");
        std::fs::write(&tmp, text.as_bytes()).map_err(|source| RaftError::Io {
            path: tmp.clone(),
            source,
        })?;
        std::fs::rename(&tmp, &self.state_path).map_err(|source| RaftError::Io {
            path: self.state_path.clone(),
            source,
        })?;
        self.hard_state = hs.clone();
        Ok(())
    }

    fn hard_state(&self) -> HardState {
        self.hard_state.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::{Command, CommandKind};
    use tempfile::tempdir;

    fn entry(term: u64, index: u64, payload: &[u8]) -> LogEntry {
        LogEntry {
            term: Term(term),
            index: LogIndex(index),
            command: Command::new(CommandKind::Database, payload.to_vec()),
        }
    }

    #[test]
    fn append_persists_after_reopen() {
        let dir = tempdir().unwrap();
        {
            let mut s = FileStorage::open(dir.path()).unwrap();
            s.append(&[entry(1, 1, b"a"), entry(1, 2, b"b")]).unwrap();
            s.save_hard_state(&HardState {
                current_term: Term(1),
                voted_for: None,
                commit_index: LogIndex(2),
            })
            .unwrap();
        }
        let s = FileStorage::open(dir.path()).unwrap();
        assert_eq!(s.last_index(), LogIndex(2));
        assert_eq!(s.hard_state().commit_index, LogIndex(2));
        assert_eq!(s.entry_at(LogIndex(2)).unwrap().command.payload, b"b");
    }

    #[test]
    fn hard_state_persists() {
        let dir = tempdir().unwrap();
        let id = auradb_cluster::NodeId::from_raw(0x99);
        {
            let mut s = FileStorage::open(dir.path()).unwrap();
            s.save_hard_state(&HardState {
                current_term: Term(7),
                voted_for: Some(id),
                commit_index: LogIndex::ZERO,
            })
            .unwrap();
        }
        let s = FileStorage::open(dir.path()).unwrap();
        assert_eq!(s.hard_state().current_term, Term(7));
        assert_eq!(s.hard_state().voted_for, Some(id));
    }

    #[test]
    fn truncate_suffix_then_reopen() {
        let dir = tempdir().unwrap();
        {
            let mut s = FileStorage::open(dir.path()).unwrap();
            s.append(&[entry(1, 1, b"a"), entry(1, 2, b"b"), entry(1, 3, b"c")])
                .unwrap();
            s.truncate_from(LogIndex(2)).unwrap();
            s.append(&[entry(2, 2, b"B")]).unwrap();
        }
        let s = FileStorage::open(dir.path()).unwrap();
        assert_eq!(s.last_index(), LogIndex(2));
        assert_eq!(s.term_at(LogIndex(2)), Some(Term(2)));
        assert_eq!(s.entry_at(LogIndex(2)).unwrap().command.payload, b"B");
    }

    #[test]
    fn corruption_is_detected() {
        let dir = tempdir().unwrap();
        {
            let mut s = FileStorage::open(dir.path()).unwrap();
            s.append(&[entry(1, 1, b"abc")]).unwrap();
        }
        // Flip a byte in the middle of the frame payload.
        let log = dir.path().join(LOG_FILE);
        let mut bytes = std::fs::read(&log).unwrap();
        let n = bytes.len();
        bytes[n - 1] ^= 0xff;
        std::fs::write(&log, bytes).unwrap();
        assert!(matches!(
            FileStorage::open(dir.path()),
            Err(RaftError::Corruption(_))
        ));
    }

    #[test]
    fn torn_trailing_frame_is_dropped() {
        let dir = tempdir().unwrap();
        {
            let mut s = FileStorage::open(dir.path()).unwrap();
            s.append(&[entry(1, 1, b"a")]).unwrap();
        }
        let log = dir.path().join(LOG_FILE);
        let mut bytes = std::fs::read(&log).unwrap();
        // Append a half-written frame header.
        bytes.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 9, 1, 2, 3]);
        std::fs::write(&log, bytes).unwrap();
        let s = FileStorage::open(dir.path()).unwrap();
        assert_eq!(s.last_index(), LogIndex(1));
    }
}
