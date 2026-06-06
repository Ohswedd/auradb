//! A durable file-backed [`RaftStorage`] with log compaction support.
//!
//! Layout (under the directory the caller chooses, e.g. `<data_dir>/cluster/`):
//!
//! ```text
//! raft-log.bin          # append-only, framed, checksummed log entries
//! raft-state.json       # the hard state (current term, vote, commit index)
//! raft-compaction.json  # the compacted prefix (last included index + term)
//! ```
//!
//! Each log frame is `[len: u64 BE][crc32: u32 BE][json bytes]`, so a torn
//! trailing frame (a crash mid-append) is detected and dropped on open, while a
//! checksum mismatch on a fully present frame fails closed as corruption — the
//! same discipline the storage engine uses for its segments.
//!
//! ## Compaction
//!
//! Once a snapshot durably covers a prefix of the log, those entries can be
//! discarded. The log records the **last included index** and **term** of the
//! compacted prefix in `raft-compaction.json`; the in-memory `entries` then holds
//! only the retained suffix. Reads at or below the prefix return
//! [`RaftError::Compacted`] rather than a wrong or empty answer, and the
//! AppendEntries consistency check still resolves [`term_at`](RaftStorage::term_at)
//! at the boundary index from the recorded term. Compaction itself never runs
//! ahead of durability: [`FileStorage::compact`] refuses to discard entries that
//! are not yet applied or that lie beyond the committed index.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{RaftError, Result};
use crate::log::{validate_suffix, HardState, LogEntry, LogIndex, RaftStorage, Term};

const LOG_FILE: &str = "raft-log.bin";
const STATE_FILE: &str = "raft-state.json";
const COMPACTION_FILE: &str = "raft-compaction.json";

/// The on-disk compaction-metadata format version this build writes and reads.
const COMPACTION_FORMAT_VERSION: u32 = 1;

/// Persisted record of the compacted prefix. A snapshot covers every index up to
/// and including `last_included_index`; the durable log retains only entries
/// strictly after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct CompactionMeta {
    format_version: u32,
    last_included_index: u64,
    last_included_term: u64,
}

/// The outcome of a compaction request (or dry run).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactionOutcome {
    /// Whether anything was (or would be) discarded.
    pub compacted: bool,
    /// Number of log entries discarded (or that would be discarded).
    pub entries_discarded: u64,
    /// The last index included in the compacted prefix afterwards.
    pub last_included_index: LogIndex,
    /// The term of `last_included_index`.
    pub last_included_term: Term,
    /// The log's last index (unchanged by compaction).
    pub last_index: LogIndex,
    /// Whether this was a dry run (no files modified).
    pub dry_run: bool,
}

/// A durable Raft log + hard state backed by files in a directory.
#[derive(Debug)]
pub struct FileStorage {
    dir: PathBuf,
    log_path: PathBuf,
    state_path: PathBuf,
    compaction_path: PathBuf,
    file: File,
    /// The retained suffix: entries with absolute indices
    /// `base_index+1 ..= base_index+entries.len()`.
    entries: Vec<LogEntry>,
    hard_state: HardState,
    /// The last index covered by a snapshot and discarded from `entries`
    /// (`0` when nothing has been compacted).
    base_index: LogIndex,
    /// The term of `base_index` (the boundary the next retained entry extends).
    base_term: Term,
}

impl FileStorage {
    /// Open (creating if absent) a durable Raft store in `dir`.
    ///
    /// Replays the log, validating checksums and the contiguity/term invariants,
    /// loads the hard state, and loads any compaction metadata. A torn trailing
    /// frame is truncated; any other integrity failure (including compaction
    /// metadata that disagrees with the retained log) is reported as
    /// [`RaftError::Corruption`].
    pub fn open(dir: impl AsRef<Path>) -> Result<FileStorage> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir).map_err(|source| RaftError::Io {
            path: dir.clone(),
            source,
        })?;
        let log_path = dir.join(LOG_FILE);
        let state_path = dir.join(STATE_FILE);
        let compaction_path = dir.join(COMPACTION_FILE);

        let (base_index, base_term) = Self::load_compaction(&compaction_path)?;
        let (entries, valid_len) = Self::replay_log(&log_path, base_index, base_term)?;

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
            compaction_path,
            file,
            entries,
            hard_state,
            base_index,
            base_term,
        })
    }

    /// The directory backing this store.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The last index covered by a snapshot and removed from the log
    /// (`LogIndex::ZERO` when nothing has been compacted).
    pub fn last_included_index(&self) -> LogIndex {
        self.base_index
    }

    /// The term of [`last_included_index`](Self::last_included_index).
    pub fn last_included_term(&self) -> Term {
        self.base_term
    }

    /// The highest index that may be safely compacted given an `applied` index.
    ///
    /// Compaction never runs ahead of durability or apply: the prefix is bounded
    /// by the committed index, the applied index the caller supplies, and the end
    /// of the log. The result is never below the current compacted prefix.
    pub fn compactable_prefix(&self, applied: LogIndex) -> LogIndex {
        let committed = self.hard_state.commit_index;
        let bound = applied
            .get()
            .min(committed.get())
            .min(self.last_index().get());
        LogIndex(bound.max(self.base_index.get()))
    }

    /// Read an entry at an absolute `index`, failing closed with
    /// [`RaftError::Compacted`] when it lies at or below the compacted prefix.
    ///
    /// Unlike [`entry_at`](RaftStorage::entry_at) (which returns `None` for an
    /// absent index), this distinguishes "compacted away" from "never existed",
    /// which is what the compaction boundary requires.
    pub fn read_at(&self, index: LogIndex) -> Result<Option<LogEntry>> {
        if index != LogIndex::ZERO && index <= self.base_index {
            return Err(RaftError::Compacted {
                requested: index.get(),
                last_included: self.base_index.get(),
            });
        }
        Ok(self.entry_at(index))
    }

    /// Compute what a compaction up to `up_to` would do, without modifying any
    /// files. Use this to preview a `compact` (the CLI dry-run path).
    pub fn compact_dry_run(&self, up_to: LogIndex, applied: LogIndex) -> Result<CompactionOutcome> {
        self.plan_compaction(up_to, applied, true)
    }

    /// Discard the log prefix up to and including `up_to`, recording it as the
    /// compacted prefix. `applied` is the highest index the caller has durably
    /// applied to its state machine (a snapshot must cover the prefix first).
    ///
    /// Refuses (with [`RaftError::CompactionRefused`]) to discard entries that are
    /// not yet applied, lie beyond the committed index, or lie beyond the end of
    /// the log. Compacting a prefix already covered is a no-op. The retained
    /// suffix is rewritten atomically and `raft-compaction.json` is persisted, so
    /// a restart sees a consistent boundary.
    pub fn compact(&mut self, up_to: LogIndex, applied: LogIndex) -> Result<CompactionOutcome> {
        let plan = self.plan_compaction(up_to, applied, false)?;
        if !plan.compacted {
            return Ok(plan);
        }
        let last_included_term = plan.last_included_term;
        // Persist the new boundary first: if the log rewrite is interrupted, a
        // boundary ahead of the retained log is detected on open and fails closed
        // rather than silently losing the invariant.
        self.write_compaction(up_to, last_included_term)?;
        self.base_index = up_to;
        self.base_term = last_included_term;
        // Entries currently cover old_base+1.. ; drop those whose absolute index
        // is within the newly compacted prefix.
        self.entries.retain(|e| e.index > up_to);
        self.rewrite_log()?;
        Ok(plan)
    }

    /// Install a snapshot boundary received from the leader (the follower side of
    /// peer snapshot install).
    ///
    /// A leader snapshot covers every index up to and including
    /// `last_included_index`; this is used by a follower that has fallen behind
    /// the leader's compacted prefix and can no longer be served by AppendEntries.
    /// The follower adopts the boundary, drops the log entries the snapshot
    /// subsumes, and persists the new boundary and commit index. Returns `true`
    /// when the boundary advanced (a stale or already-covered snapshot is a no-op
    /// returning `false`).
    ///
    /// If the follower already holds an entry at `last_included_index` whose term
    /// matches `last_included_term`, only the subsumed prefix is dropped and the
    /// suffix after the boundary is retained (the standard Raft rule). Otherwise
    /// the entire log is discarded because it conflicts with the snapshot the
    /// leader has committed. The boundary is persisted before the log is rewritten,
    /// so an interrupted install fails closed on reopen rather than leaving a
    /// boundary ahead of the retained log.
    pub fn install_snapshot(
        &mut self,
        last_included_index: LogIndex,
        last_included_term: Term,
    ) -> Result<bool> {
        if last_included_index <= self.base_index {
            // An equal-or-older snapshot: the prefix is already covered.
            return Ok(false);
        }
        let retain_suffix =
            matches!(self.term_at(last_included_index), Some(t) if t == last_included_term);
        // Persist the new boundary first (same crash discipline as `compact`).
        self.write_compaction(last_included_index, last_included_term)?;
        if retain_suffix {
            self.entries.retain(|e| e.index > last_included_index);
        } else {
            self.entries.clear();
        }
        self.base_index = last_included_index;
        self.base_term = last_included_term;
        self.rewrite_log()?;
        // The snapshot is durable committed state up to the boundary: advance the
        // persisted commit index so the boundary stays consistent across restarts.
        if self.hard_state.commit_index < last_included_index {
            let mut hs = self.hard_state.clone();
            hs.commit_index = last_included_index;
            self.save_hard_state(&hs)?;
        }
        Ok(true)
    }

    /// Validate a compaction request and return its outcome. Shared by the dry
    /// run and the real compaction so both agree exactly.
    fn plan_compaction(
        &self,
        up_to: LogIndex,
        applied: LogIndex,
        dry_run: bool,
    ) -> Result<CompactionOutcome> {
        let last = self.last_index();
        if up_to > last {
            return Err(RaftError::CompactionRefused(format!(
                "last included index {up_to} is beyond the end of the log (last index {last})"
            )));
        }
        if up_to > self.hard_state.commit_index {
            return Err(RaftError::CompactionRefused(format!(
                "last included index {up_to} is beyond the committed index {}",
                self.hard_state.commit_index
            )));
        }
        if up_to > applied {
            return Err(RaftError::CompactionRefused(format!(
                "last included index {up_to} has not been applied yet (applied index {applied})"
            )));
        }
        if up_to <= self.base_index {
            // Nothing new to discard: the prefix is already covered.
            return Ok(CompactionOutcome {
                compacted: false,
                entries_discarded: 0,
                last_included_index: self.base_index,
                last_included_term: self.base_term,
                last_index: last,
                dry_run,
            });
        }
        let last_included_term = self.term_at(up_to).ok_or_else(|| {
            RaftError::CompactionRefused(format!(
                "cannot resolve the term of last included index {up_to}"
            ))
        })?;
        let discarded = up_to.get() - self.base_index.get();
        Ok(CompactionOutcome {
            compacted: true,
            entries_discarded: discarded,
            last_included_index: up_to,
            last_included_term,
            last_index: last,
            dry_run,
        })
    }

    fn load_compaction(path: &Path) -> Result<(LogIndex, Term)> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let meta: CompactionMeta = serde_json::from_str(&text).map_err(|e| {
                    RaftError::Corruption(format!("malformed raft compaction metadata: {e}"))
                })?;
                if meta.format_version > COMPACTION_FORMAT_VERSION {
                    return Err(RaftError::Corruption(format!(
                        "raft compaction metadata declares format version {} but this build \
                         supports up to {COMPACTION_FORMAT_VERSION}",
                        meta.format_version
                    )));
                }
                Ok((
                    LogIndex(meta.last_included_index),
                    Term(meta.last_included_term),
                ))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((LogIndex::ZERO, Term::ZERO)),
            Err(source) => Err(RaftError::Io {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    fn write_compaction(
        &self,
        last_included_index: LogIndex,
        last_included_term: Term,
    ) -> Result<()> {
        let meta = CompactionMeta {
            format_version: COMPACTION_FORMAT_VERSION,
            last_included_index: last_included_index.get(),
            last_included_term: last_included_term.get(),
        };
        let text = serde_json::to_string_pretty(&meta)
            .map_err(|e| RaftError::Codec(format!("encoding compaction metadata: {e}")))?;
        let tmp = self.compaction_path.with_extension("json.tmp");
        std::fs::write(&tmp, text.as_bytes()).map_err(|source| RaftError::Io {
            path: tmp.clone(),
            source,
        })?;
        std::fs::rename(&tmp, &self.compaction_path).map_err(|source| RaftError::Io {
            path: self.compaction_path.clone(),
            source,
        })
    }

    /// Replay the durable log into memory. `base_index`/`base_term` describe the
    /// compacted prefix the retained log is expected to extend; the first frame
    /// must be at `base_index + 1`, or the metadata and log disagree (corruption).
    fn replay_log(
        path: &Path,
        base_index: LogIndex,
        base_term: Term,
    ) -> Result<(Vec<LogEntry>, usize)> {
        let mut buf = Vec::new();
        match File::open(path) {
            Ok(mut f) => {
                f.read_to_end(&mut buf).map_err(|source| RaftError::Io {
                    path: path.to_path_buf(),
                    source,
                })?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // No log file: valid only if nothing was retained past the prefix.
                return Ok((Vec::new(), 0));
            }
            Err(source) => {
                return Err(RaftError::Io {
                    path: path.to_path_buf(),
                    source,
                })
            }
        }

        let mut entries: Vec<LogEntry> = Vec::new();
        let mut offset = 0usize;
        let mut last_index = base_index;
        let mut last_term = base_term;
        let mut first = true;
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
            if first && base_index != LogIndex::ZERO && entry.index != base_index.next() {
                return Err(RaftError::Corruption(format!(
                    "raft compaction metadata (last included index {base_index}) disagrees with \
                     the retained log (first entry index {})",
                    entry.index
                )));
            }
            first = false;
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

    /// The position of an absolute `index` within the retained `entries` vector,
    /// or `None` if it is compacted away or past the end.
    fn slot(&self, index: LogIndex) -> Option<usize> {
        if index <= self.base_index {
            return None;
        }
        let rel = (index.get() - self.base_index.get() - 1) as usize;
        (rel < self.entries.len()).then_some(rel)
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
        // Truncating into the compacted prefix would discard committed,
        // snapshot-covered history: fail closed rather than corrupt the boundary.
        if index != LogIndex::ZERO && index <= self.base_index {
            return Err(RaftError::Compacted {
                requested: index.get(),
                last_included: self.base_index.get(),
            });
        }
        let keep_rel = index
            .get()
            .saturating_sub(self.base_index.get())
            .saturating_sub(1) as usize;
        if keep_rel < self.entries.len() {
            self.entries.truncate(keep_rel);
            self.rewrite_log()?;
        }
        Ok(())
    }

    fn term_at(&self, index: LogIndex) -> Option<Term> {
        if index == LogIndex::ZERO {
            return Some(Term::ZERO);
        }
        if index == self.base_index {
            // The boundary term is known even though the entry itself is gone, so
            // the AppendEntries consistency check resolves across the prefix.
            return Some(self.base_term);
        }
        self.slot(index).map(|rel| self.entries[rel].term)
    }

    fn entry_at(&self, index: LogIndex) -> Option<LogEntry> {
        self.slot(index).map(|rel| self.entries[rel].clone())
    }

    fn entries_from(&self, index: LogIndex) -> Vec<LogEntry> {
        // Clamp the start to the first retained entry; entries below the prefix
        // are not present, so callers receive the available suffix.
        let start_abs = index.get().max(self.base_index.get() + 1);
        match self.slot(LogIndex(start_abs)) {
            Some(rel) => self.entries[rel..].to_vec(),
            None => Vec::new(),
        }
    }

    fn last_index(&self) -> LogIndex {
        LogIndex(self.base_index.get() + self.entries.len() as u64)
    }

    fn last_term(&self) -> Term {
        self.entries
            .last()
            .map(|e| e.term)
            .unwrap_or(self.base_term)
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

    fn seed(dir: &Path, count: u64, commit: u64) -> FileStorage {
        let mut s = FileStorage::open(dir).unwrap();
        let entries: Vec<LogEntry> = (1..=count).map(|i| entry(1, i, &[i as u8])).collect();
        s.append(&entries).unwrap();
        s.save_hard_state(&HardState {
            current_term: Term(1),
            voted_for: None,
            commit_index: LogIndex(commit),
        })
        .unwrap();
        s
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

    // ---- compaction ----

    #[test]
    fn compaction_refuses_uncommitted_entries() {
        let dir = tempdir().unwrap();
        let mut s = seed(dir.path(), 5, 3); // commit = 3
                                            // up_to within applied but beyond the committed index is refused.
        let err = s.compact(LogIndex(4), LogIndex(4)).unwrap_err();
        assert!(matches!(err, RaftError::CompactionRefused(_)));
    }

    #[test]
    fn compaction_refuses_unapplied_entries() {
        let dir = tempdir().unwrap();
        let mut s = seed(dir.path(), 5, 5); // commit = 5
                                            // Committed but not yet applied past index 2.
        let err = s.compact(LogIndex(4), LogIndex(2)).unwrap_err();
        assert!(matches!(err, RaftError::CompactionRefused(_)));
    }

    #[test]
    fn compaction_records_last_included_index_and_term() {
        let dir = tempdir().unwrap();
        let mut s = seed(dir.path(), 5, 5);
        let out = s.compact(LogIndex(3), LogIndex(5)).unwrap();
        assert!(out.compacted);
        assert_eq!(out.entries_discarded, 3);
        assert_eq!(s.last_included_index(), LogIndex(3));
        assert_eq!(s.last_included_term(), Term(1));
        // The retained suffix and the boundary are intact.
        assert_eq!(s.last_index(), LogIndex(5));
        assert_eq!(s.term_at(LogIndex(3)), Some(Term(1)));
        assert_eq!(s.entry_at(LogIndex(4)).unwrap().command.payload, &[4u8]);
    }

    #[test]
    fn read_before_prefix_returns_compacted() {
        let dir = tempdir().unwrap();
        let mut s = seed(dir.path(), 5, 5);
        s.compact(LogIndex(3), LogIndex(5)).unwrap();
        assert!(matches!(
            s.read_at(LogIndex(2)),
            Err(RaftError::Compacted {
                requested: 2,
                last_included: 3
            })
        ));
        // A retained index reads back normally.
        assert!(s.read_at(LogIndex(4)).unwrap().is_some());
    }

    #[test]
    fn compaction_persists_after_restart() {
        let dir = tempdir().unwrap();
        {
            let mut s = seed(dir.path(), 5, 5);
            s.compact(LogIndex(3), LogIndex(5)).unwrap();
        }
        let s = FileStorage::open(dir.path()).unwrap();
        assert_eq!(s.last_included_index(), LogIndex(3));
        assert_eq!(s.last_included_term(), Term(1));
        assert_eq!(s.last_index(), LogIndex(5));
        assert!(matches!(
            s.read_at(LogIndex(1)),
            Err(RaftError::Compacted { .. })
        ));
        assert_eq!(s.entry_at(LogIndex(5)).unwrap().command.payload, &[5u8]);
    }

    #[test]
    fn compaction_corrupt_metadata_rejected() {
        let dir = tempdir().unwrap();
        {
            let mut s = seed(dir.path(), 5, 5);
            s.compact(LogIndex(3), LogIndex(5)).unwrap();
        }
        // A future compaction format version fails closed on open.
        std::fs::write(
            dir.path().join(COMPACTION_FILE),
            br#"{"format_version": 9999, "last_included_index": 3, "last_included_term": 1}"#,
        )
        .unwrap();
        assert!(matches!(
            FileStorage::open(dir.path()),
            Err(RaftError::Corruption(_))
        ));
    }

    #[test]
    fn compaction_metadata_disagreeing_with_log_rejected() {
        let dir = tempdir().unwrap();
        {
            let mut s = seed(dir.path(), 5, 5);
            s.compact(LogIndex(3), LogIndex(5)).unwrap();
        }
        // Claim a prefix that does not line up with the retained first entry (4).
        std::fs::write(
            dir.path().join(COMPACTION_FILE),
            br#"{"format_version": 1, "last_included_index": 2, "last_included_term": 1}"#,
        )
        .unwrap();
        assert!(matches!(
            FileStorage::open(dir.path()),
            Err(RaftError::Corruption(_))
        ));
    }

    #[test]
    fn compaction_already_covered_is_noop() {
        let dir = tempdir().unwrap();
        let mut s = seed(dir.path(), 5, 5);
        s.compact(LogIndex(3), LogIndex(5)).unwrap();
        let again = s.compact(LogIndex(2), LogIndex(5)).unwrap();
        assert!(!again.compacted);
        assert_eq!(s.last_included_index(), LogIndex(3));
    }

    #[test]
    fn compactable_prefix_bounded_by_commit_and_applied() {
        let dir = tempdir().unwrap();
        let s = seed(dir.path(), 5, 3); // commit = 3
        assert_eq!(s.compactable_prefix(LogIndex(5)), LogIndex(3));
        assert_eq!(s.compactable_prefix(LogIndex(2)), LogIndex(2));
    }

    #[test]
    fn append_after_compaction_extends_boundary() {
        let dir = tempdir().unwrap();
        let mut s = seed(dir.path(), 3, 3);
        s.compact(LogIndex(3), LogIndex(3)).unwrap();
        // The log is now empty but the boundary is index 3 / term 1.
        assert_eq!(s.last_index(), LogIndex(3));
        s.append(&[entry(1, 4, b"d")]).unwrap();
        assert_eq!(s.last_index(), LogIndex(4));
        assert_eq!(s.entry_at(LogIndex(4)).unwrap().command.payload, b"d");
        // A gap append is still rejected.
        assert!(s.append(&[entry(1, 6, b"x")]).is_err());
    }

    #[test]
    fn install_snapshot_jumps_boundary_ahead_of_log() {
        let dir = tempdir().unwrap();
        let mut s = seed(dir.path(), 3, 3); // follower has entries 1..=3
                                            // A leader snapshot covers up to index 10 (entries the follower lacks).
        let installed = s.install_snapshot(LogIndex(10), Term(4)).unwrap();
        assert!(installed);
        assert_eq!(s.last_included_index(), LogIndex(10));
        assert_eq!(s.last_included_term(), Term(4));
        // The whole prior log is subsumed; the boundary becomes the new end.
        assert_eq!(s.last_index(), LogIndex(10));
        assert_eq!(s.term_at(LogIndex(10)), Some(Term(4)));
        assert!(matches!(
            s.read_at(LogIndex(3)),
            Err(RaftError::Compacted { .. })
        ));
        // The commit index advanced to the boundary and AppendEntries can resume.
        assert_eq!(s.hard_state().commit_index, LogIndex(10));
        s.append(&[entry(4, 11, b"k")]).unwrap();
        assert_eq!(s.last_index(), LogIndex(11));
    }

    #[test]
    fn install_snapshot_persists_after_restart() {
        let dir = tempdir().unwrap();
        {
            let mut s = seed(dir.path(), 3, 3);
            s.install_snapshot(LogIndex(10), Term(4)).unwrap();
        }
        let s = FileStorage::open(dir.path()).unwrap();
        assert_eq!(s.last_included_index(), LogIndex(10));
        assert_eq!(s.last_included_term(), Term(4));
        assert_eq!(s.last_index(), LogIndex(10));
        assert_eq!(s.hard_state().commit_index, LogIndex(10));
    }

    #[test]
    fn install_snapshot_stale_boundary_is_noop() {
        let dir = tempdir().unwrap();
        let mut s = seed(dir.path(), 5, 5);
        s.compact(LogIndex(3), LogIndex(5)).unwrap();
        // A snapshot at or below the current prefix changes nothing.
        let installed = s.install_snapshot(LogIndex(2), Term(1)).unwrap();
        assert!(!installed);
        assert_eq!(s.last_included_index(), LogIndex(3));
        assert_eq!(s.last_index(), LogIndex(5));
    }

    #[test]
    fn install_snapshot_retains_matching_suffix() {
        let dir = tempdir().unwrap();
        let mut s = seed(dir.path(), 5, 5); // entries 1..=5 at term 1
                                            // A snapshot at index 3 / term 1 matches the existing entry 3, so the
                                            // suffix 4..=5 survives.
        let installed = s.install_snapshot(LogIndex(3), Term(1)).unwrap();
        assert!(installed);
        assert_eq!(s.last_included_index(), LogIndex(3));
        assert_eq!(s.last_index(), LogIndex(5));
        assert_eq!(s.entry_at(LogIndex(4)).unwrap().command.payload, &[4u8]);
    }

    #[test]
    fn dry_run_does_not_modify_files() {
        let dir = tempdir().unwrap();
        let s = seed(dir.path(), 5, 5);
        let preview = s.compact_dry_run(LogIndex(3), LogIndex(5)).unwrap();
        assert!(preview.compacted);
        assert!(preview.dry_run);
        assert_eq!(preview.entries_discarded, 3);
        // Nothing changed: no compaction file, full log still present.
        assert!(!dir.path().join(COMPACTION_FILE).exists());
        assert_eq!(s.last_included_index(), LogIndex::ZERO);
        assert!(s.read_at(LogIndex(1)).unwrap().is_some());
    }
}
