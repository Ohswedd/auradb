//! On-disk persistence for collection indexes.
//!
//! Indexes are persisted as deterministic snapshots under an `indexes/`
//! directory in the database data directory. Each snapshot file is
//! self-describing and integrity-checked:
//!
//! ```text
//! offset  field
//! 0..4    magic "AIDX"
//! 4..8    format version (u32, big-endian)
//! 8..12   payload length (u32, big-endian)
//! 12..16  CRC32 of the payload (u32, big-endian)
//! 16..    payload (JSON-encoded IndexSnapshot)
//! ```
//!
//! A snapshot records the content [`fingerprint`](crate::fingerprint) of the
//! collection it was built from. On open, the engine recomputes the fingerprint
//! from storage; a snapshot is used only when its fingerprint, schema field
//! shape, and CRC all match, and is otherwise safely rebuilt from storage. This
//! is a snapshot-plus-checkpoint model: snapshots are written at checkpoints
//! (flush, compaction, graceful shutdown, and `auradb index rebuild`), and any
//! divergence is detected and repaired on the next open.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use auradb_core::{Error, RecordId, Result};
use serde::{Deserialize, Serialize};

/// The current on-disk index snapshot format version.
pub const INDEX_FORMAT_VERSION: u32 = 1;

const MAGIC: [u8; 4] = *b"AIDX";
const HEADER_LEN: usize = 16;

/// The manifest file name within the index directory.
pub const INDEX_MANIFEST_FILE: &str = "INDEX_MANIFEST.json";

/// A persisted unique (or primary-key) index: value key to record id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UniqueIndexData {
    /// The indexed field name.
    pub field: String,
    /// `(value_key, record_id)` entries.
    pub entries: Vec<(String, RecordId)>,
}

/// A persisted secondary (non-unique) index: value key to record ids.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecondaryIndexData {
    /// The indexed field name (or dotted document path).
    pub field: String,
    /// `(value_key, [record_id, ...])` entries.
    pub entries: Vec<(String, Vec<RecordId>)>,
}

/// A persisted exact vector index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorIndexData {
    /// The vector field name.
    pub field: String,
    /// The vector dimensionality.
    pub dim: usize,
    /// `(record_id, vector)` entries.
    pub entries: Vec<(RecordId, Vec<f32>)>,
}

/// A persisted full-text inverted index: term to record-id postings with the
/// per-record term frequency, plus the per-document field length needed for
/// BM25 length normalization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextIndexData {
    /// The text field name.
    pub field: String,
    /// `(term, [(record_id, term_frequency), ...])` postings.
    pub postings: Vec<(String, Vec<(RecordId, u32)>)>,
    /// `(record_id, field_token_length)` entries used for BM25 length
    /// normalization. Absent in snapshots written before BM25 ranking; when
    /// missing it is rebuilt from the postings on open (every document length is
    /// the sum of its term frequencies).
    #[serde(default)]
    pub doc_lengths: Vec<(RecordId, u32)>,
}

/// A serializable snapshot of one collection's indexes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexSnapshot {
    /// Snapshot format version.
    pub format_version: u32,
    /// The schema catalog version this snapshot was built against (diagnostic).
    pub schema_version: u64,
    /// The content fingerprint of the collection at snapshot time.
    pub fingerprint: u64,
    /// The primary key field, if any.
    #[serde(default)]
    pub primary_field: Option<String>,
    /// Unique / primary-key indexes.
    #[serde(default)]
    pub unique: Vec<UniqueIndexData>,
    /// Secondary equality indexes (scalar fields).
    #[serde(default)]
    pub secondary: Vec<SecondaryIndexData>,
    /// Document-path equality indexes (dotted paths).
    #[serde(default)]
    pub document_paths: Vec<SecondaryIndexData>,
    /// Exact vector indexes.
    #[serde(default)]
    pub vectors: Vec<VectorIndexData>,
    /// Full-text inverted indexes.
    #[serde(default)]
    pub text: Vec<TextIndexData>,
}

/// The index directory manifest: format version and collection-to-file map.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IndexManifest {
    /// Manifest format version.
    #[serde(default)]
    pub format_version: u32,
    /// Map of collection name to its snapshot file name.
    #[serde(default)]
    pub files: BTreeMap<String, String>,
}

/// The deterministic snapshot file name for a collection.
pub fn index_filename(collection: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in collection.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}.idx")
}

/// The path to the index manifest within `dir`.
pub fn manifest_path(dir: &Path) -> PathBuf {
    dir.join(INDEX_MANIFEST_FILE)
}

/// Load the index manifest, returning `None` if it is absent or unreadable
/// (the caller then rebuilds every index).
pub fn load_manifest(dir: &Path) -> Option<IndexManifest> {
    let bytes = fs::read(manifest_path(dir)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Persist the index manifest atomically (temp file plus rename).
pub fn save_manifest(dir: &Path, manifest: &IndexManifest) -> Result<()> {
    fs::create_dir_all(dir)?;
    let bytes = serde_json::to_vec_pretty(manifest)
        .map_err(|e| Error::Storage(format!("index manifest serialize: {e}")))?;
    let tmp = dir.join(format!("{INDEX_MANIFEST_FILE}.tmp"));
    fs::write(&tmp, &bytes)?;
    fs::rename(&tmp, manifest_path(dir))?;
    Ok(())
}

/// Write an index snapshot file atomically with a framed header and CRC.
pub fn write_snapshot(path: &Path, snapshot: &IndexSnapshot) -> Result<()> {
    let payload = serde_json::to_vec(snapshot)
        .map_err(|e| Error::Storage(format!("index serialize: {e}")))?;
    let crc = crc32fast::hash(&payload);
    let mut buf = Vec::with_capacity(HEADER_LEN + payload.len());
    buf.extend_from_slice(&MAGIC);
    buf.extend_from_slice(&INDEX_FORMAT_VERSION.to_be_bytes());
    buf.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    buf.extend_from_slice(&crc.to_be_bytes());
    buf.extend_from_slice(&payload);
    let tmp = path.with_extension("idx.tmp");
    fs::write(&tmp, &buf)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Read and validate an index snapshot file. Returns an [`Error::Corruption`]
/// for any framing, version, or checksum problem so the caller can rebuild.
pub fn read_snapshot(path: &Path) -> Result<IndexSnapshot> {
    let buf = fs::read(path)?;
    if buf.len() < HEADER_LEN || buf[0..4] != MAGIC {
        return Err(Error::Corruption("index file has bad magic".into()));
    }
    let version = u32::from_be_bytes(buf[4..8].try_into().unwrap());
    if version != INDEX_FORMAT_VERSION {
        return Err(Error::Corruption(format!(
            "unsupported index format version {version}"
        )));
    }
    let len = u32::from_be_bytes(buf[8..12].try_into().unwrap()) as usize;
    let crc = u32::from_be_bytes(buf[12..16].try_into().unwrap());
    let payload = buf
        .get(HEADER_LEN..HEADER_LEN + len)
        .ok_or_else(|| Error::Corruption("index file is truncated".into()))?;
    if crc32fast::hash(payload) != crc {
        return Err(Error::Corruption("index file checksum mismatch".into()));
    }
    serde_json::from_slice(payload).map_err(|e| Error::Corruption(format!("index decode: {e}")))
}
