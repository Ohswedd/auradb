//! Stable ranked-search pagination via opaque keyset cursor tokens (v1.2.0).
//!
//! Ranked retrieval (BM25, hybrid, exact vector) has a *total* deterministic
//! order: `score` descending, ties broken by `id` ascending. That lets us page
//! by **keyset (seek) pagination** instead of server-held offset state: a cursor
//! token carries the last row's ranking key `(score, id)` plus the rank reached
//! so far, and the next page re-evaluates the ranked query and resumes at the
//! first row ordered strictly after that key.
//!
//! The token is:
//! - **opaque and bounded** — a fixed 37-byte record (version, rank offset, id,
//!   score, query fingerprint), hex-encoded to 74 chars regardless of query size;
//! - **not secret-bearing** — it contains no query text, vector, filter, or auth
//!   material, only the continuation key and a non-reversible fingerprint of the
//!   query it belongs to (so a token cannot be replayed against a different
//!   query, and the query payload is never echoed back to the client).
//!
//! The ranking key uses the internal record id for its tie-break, not any
//! user-facing field, so the order is total and reproducible.
//!
//! Pages are duplicate-free and gap-free **when the score of the already-paged
//! rows is stable** between calls. Exact-vector similarity is corpus-independent,
//! so vector cursors are duplicate-free even across concurrent writes outside a
//! transaction. BM25 scores depend on corpus statistics (`N`, average document
//! length), so an insert/delete between pages re-scores every document and a
//! previously paged row can shift relative to the cursor key. For
//! duplicate-stable BM25/hybrid paging across writes, page inside a transaction:
//! the snapshot fixes the corpus, so the order — and therefore the cursor — is
//! fully stable. The evaluation is otherwise read-committed: a row inserted after
//! the cursor key simply appears on a later page.

use auradb_core::{Error, RecordId, Result};

use crate::exec::{execute_find_within, DataSource, Deadline, Scored};
use crate::ir::FindQuery;

/// Cursor token format version. Bumped if the encoded layout ever changes.
const CURSOR_VERSION: u8 = 1;
/// Fixed encoded length: version(1) + rank_offset(8) + id(16) + score(4) + fp(8).
const CURSOR_BYTES: usize = 1 + 8 + 16 + 4 + 8;

/// One page of ranked results plus the token to fetch the next page.
#[derive(Debug, Clone, PartialEq)]
pub struct RankedPage {
    /// The scored rows in this page, in ranked order.
    pub rows: Vec<Scored>,
    /// The opaque token for the next page, or `None` when this is the last page.
    pub next_cursor: Option<String>,
    /// The 0-based rank offset of this page within the full result, so callers
    /// can render stable 1-based ranks across pages.
    pub start_rank: usize,
}

/// The decoded continuation state carried by a cursor token.
struct CursorState {
    rank_offset: u64,
    last_id: RecordId,
    last_score: f32,
    fingerprint: u64,
}

/// FNV-1a hash, used to bind a token to the query that produced it.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// A non-reversible fingerprint of the ranking-relevant parts of a query: the
/// collection and the retrieval clause(s)/filter. A token only validates against
/// a query with the same fingerprint, so it cannot be replayed against a
/// different query. The query payload itself is never stored in the token.
fn query_fingerprint(q: &FindQuery) -> u64 {
    #[derive(serde::Serialize)]
    struct Key<'a> {
        collection: &'a str,
        filter: &'a Option<crate::ir::Filter>,
        vector: &'a Option<crate::ir::VectorSearch>,
        text_search: &'a Option<Box<crate::ir::TextSearch>>,
        hybrid: &'a Option<Box<crate::ir::HybridSearch>>,
    }
    let key = Key {
        collection: &q.collection,
        filter: &q.filter,
        vector: &q.vector,
        text_search: &q.text_search,
        hybrid: &q.hybrid,
    };
    let canonical = serde_json::to_vec(&key).unwrap_or_default();
    fnv1a(&canonical)
}

fn encode_token(c: &CursorState) -> String {
    let mut buf = [0u8; CURSOR_BYTES];
    buf[0] = CURSOR_VERSION;
    buf[1..9].copy_from_slice(&c.rank_offset.to_be_bytes());
    buf[9..25].copy_from_slice(&c.last_id.to_bytes());
    buf[25..29].copy_from_slice(&c.last_score.to_bits().to_be_bytes());
    buf[29..37].copy_from_slice(&c.fingerprint.to_be_bytes());
    let mut s = String::with_capacity(CURSOR_BYTES * 2);
    for b in buf {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        s.push(char::from_digit((b & 0xf) as u32, 16).unwrap());
    }
    s
}

fn decode_token(token: &str) -> Result<CursorState> {
    let invalid = || Error::InvalidRequest("invalid cursor token".into());
    if token.len() != CURSOR_BYTES * 2 {
        return Err(invalid());
    }
    let mut buf = [0u8; CURSOR_BYTES];
    let bytes = token.as_bytes();
    for (i, slot) in buf.iter_mut().enumerate() {
        let hi = (bytes[i * 2] as char).to_digit(16).ok_or_else(invalid)?;
        let lo = (bytes[i * 2 + 1] as char)
            .to_digit(16)
            .ok_or_else(invalid)?;
        *slot = ((hi << 4) | lo) as u8;
    }
    if buf[0] != CURSOR_VERSION {
        return Err(Error::InvalidRequest(format!(
            "unsupported cursor token version {}",
            buf[0]
        )));
    }
    let mut id_bytes = [0u8; 16];
    id_bytes.copy_from_slice(&buf[9..25]);
    Ok(CursorState {
        rank_offset: u64::from_be_bytes(buf[1..9].try_into().unwrap()),
        last_id: RecordId::from_bytes(id_bytes),
        last_score: f32::from_bits(u32::from_be_bytes(buf[25..29].try_into().unwrap())),
        fingerprint: u64::from_be_bytes(buf[29..37].try_into().unwrap()),
    })
}

/// Whether `s` is ordered strictly *after* the cursor key under the ranked order
/// (`score` desc, ties by `id` asc) — i.e. it belongs on a later page.
fn after_key(s: &Scored, last_score: f32, last_id: RecordId) -> bool {
    let sc = s.score.unwrap_or(f32::NEG_INFINITY);
    match sc
        .partial_cmp(&last_score)
        .unwrap_or(std::cmp::Ordering::Equal)
    {
        std::cmp::Ordering::Less => true,
        std::cmp::Ordering::Greater => false,
        std::cmp::Ordering::Equal => s.id > last_id,
    }
}

/// Page a ranked query (vector / text_search / hybrid) by keyset cursor.
///
/// `page_size` bounds the rows returned. `cursor` is `None` for the first page,
/// or a token returned by a previous call. Non-ranked queries are rejected — use
/// ordinary `limit`/`offset` (or server cursors) for those.
pub fn paginate_ranked(
    ds: &dyn DataSource,
    query: &FindQuery,
    page_size: usize,
    cursor: Option<&str>,
    deadline: &Deadline,
) -> Result<RankedPage> {
    if page_size == 0 {
        return Err(Error::InvalidRequest("page_size must be >= 1".into()));
    }
    let ranked = query.vector.is_some() || query.text_search.is_some() || query.hybrid.is_some();
    if !ranked {
        return Err(Error::InvalidRequest(
            "ranked cursor requires a vector, text_search, or hybrid clause".into(),
        ));
    }
    let fingerprint = query_fingerprint(query);

    // Evaluate the full ranked order (the query's own offset/limit do not apply
    // to cursor paging; a hybrid `top_k` or vector `k` still bounds the set).
    let mut full = query.clone();
    full.offset = None;
    full.limit = None;
    let ordered = execute_find_within(ds, &full, deadline)?.ordered;

    let (start, base_rank) = match cursor {
        None => (0usize, 0usize),
        Some(token) => {
            let st = decode_token(token)?;
            if st.fingerprint != fingerprint {
                return Err(Error::InvalidRequest(
                    "cursor token does not belong to this query".into(),
                ));
            }
            let idx = ordered
                .iter()
                .position(|s| after_key(s, st.last_score, st.last_id))
                .unwrap_or(ordered.len());
            (idx, st.rank_offset as usize)
        }
    };

    let end = (start + page_size).min(ordered.len());
    let rows: Vec<Scored> = ordered[start..end].to_vec();
    let next_cursor = if end < ordered.len() {
        let last = &ordered[end - 1];
        Some(encode_token(&CursorState {
            rank_offset: (base_rank + rows.len()) as u64,
            last_id: last.id,
            last_score: last.score.unwrap_or(f32::NEG_INFINITY),
            fingerprint,
        }))
    } else {
        None
    };

    Ok(RankedPage {
        rows,
        next_cursor,
        start_rank: base_rank,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_roundtrips_and_is_bounded_and_opaque() {
        let st = CursorState {
            rank_offset: 42,
            last_id: RecordId::from_u128(0x1234_5678_9abc),
            last_score: 1.5,
            fingerprint: 0xdead_beef,
        };
        let token = encode_token(&st);
        // Fixed, bounded length regardless of query size.
        assert_eq!(token.len(), CURSOR_BYTES * 2);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
        let back = decode_token(&token).unwrap();
        assert_eq!(back.rank_offset, 42);
        assert_eq!(back.last_id, RecordId::from_u128(0x1234_5678_9abc));
        assert_eq!(back.last_score, 1.5);
        assert_eq!(back.fingerprint, 0xdead_beef);
    }

    #[test]
    fn malformed_tokens_are_rejected() {
        assert!(decode_token("not-hex").is_err());
        assert!(decode_token("zz").is_err());
        // Right length, wrong version byte.
        let mut s = encode_token(&CursorState {
            rank_offset: 0,
            last_id: RecordId::from_u128(1),
            last_score: 0.0,
            fingerprint: 0,
        });
        s.replace_range(0..2, "ff");
        assert!(decode_token(&s).is_err());
    }
}
