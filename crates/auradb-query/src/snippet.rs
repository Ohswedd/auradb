//! Deterministic highlight / snippet builder (foundation).
//!
//! This module produces plain-text snippet fragments with highlight offsets from
//! *already-retrieved* document text. It is the safe core of a future server-side
//! snippet surface: it is deterministic, it operates only on explicitly allowed
//! fields, and it caps both fragment length and fragment count so it can never
//! echo an entire large document or a field the caller did not request.
//!
//! What it deliberately does **not** do in this slice: it is not yet wired through
//! the Aura Wire Protocol or the server dispatch path. The over-the-wire snippet
//! response (and its capability negotiation) is the next step — see the design note
//! in `docs/SEARCH_AND_RANKING.md`. Shipping the deterministic core first, fully
//! tested for field-allowlisting and caps, keeps the wire surface honest when it
//! lands.
//!
//! Output is **plain text**: fragments carry the original characters plus byte
//! ranges marking the matched spans. Callers that render to HTML must escape the
//! text themselves; this module makes no markup claims.

use std::collections::BTreeMap;

use auradb_index::Analyzer;

/// A highlighted byte range *within a fragment's text* (`start..end`, byte offsets
/// into [`SnippetFragment::text`], not the source document).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HighlightRange {
    /// Inclusive byte offset into the fragment text where the match begins.
    pub start: usize,
    /// Exclusive byte offset into the fragment text where the match ends.
    pub end: usize,
}

/// One snippet fragment: a slice of the source field plus the highlighted ranges
/// inside it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SnippetFragment {
    /// The plain-text fragment, a contiguous slice of the source field.
    pub text: String,
    /// The highlighted ranges within [`text`](SnippetFragment::text), in order.
    pub ranges: Vec<HighlightRange>,
}

/// A snippet for one field of one result document.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Snippet {
    /// The field the snippet was built from (always one of the allowed fields).
    pub field: String,
    /// The fragments, deterministic in order. Empty when the query did not match.
    pub fragments: Vec<SnippetFragment>,
}

/// Caps and limits for snippet construction. Defaults are conservative.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnippetOptions {
    /// The maximum number of fragments returned for a field.
    pub max_fragments: usize,
    /// The maximum length, in characters, of a single fragment.
    pub max_fragment_chars: usize,
}

impl Default for SnippetOptions {
    fn default() -> Self {
        SnippetOptions {
            max_fragments: 3,
            max_fragment_chars: 200,
        }
    }
}

/// Build a snippet for `field`, highlighting where `query` matches under `analyzer`.
///
/// `fields` maps a document's exposed text fields to their text. `allowed_fields`
/// is the caller's allowlist: a field is only ever read or returned if it appears
/// there. This is the redaction boundary — a field absent from `allowed_fields` is
/// never touched, so internal or unrequested fields cannot leak into a snippet.
///
/// Returns:
/// * `None` when `field` is not in `allowed_fields`, or is absent from `fields`.
/// * `Some(Snippet { fragments: [] })` when the field is present and allowed but the
///   query did not match (including an empty query).
/// * `Some(Snippet { .. })` with up to `opts.max_fragments` fragments otherwise.
pub fn build_snippet(
    field: &str,
    allowed_fields: &[&str],
    fields: &BTreeMap<String, String>,
    query: &str,
    analyzer: Analyzer,
    opts: &SnippetOptions,
) -> Option<Snippet> {
    // Redaction boundary: refuse anything outside the explicit allowlist.
    if !allowed_fields.contains(&field) {
        return None;
    }
    let text = fields.get(field)?;

    // The set of query token texts under this analyzer. Matching is by analyzed
    // token text, so folding/lowercasing applies symmetrically to query and field.
    let query_terms: std::collections::HashSet<String> = analyzer
        .analyze(query)
        .into_iter()
        .map(|t| t.text)
        .collect();

    let fragments = if query_terms.is_empty() {
        Vec::new()
    } else {
        let matched: Vec<(usize, usize)> = analyzer
            .analyze(text)
            .into_iter()
            .filter(|t| query_terms.contains(&t.text))
            .map(|t| (t.start, t.end))
            .collect();
        build_fragments(text, &matched, opts)
    };

    Some(Snippet {
        field: field.to_string(),
        fragments,
    })
}

/// Assemble fragments from the matched source byte spans, honoring the caps.
///
/// Short fields (within the per-fragment char budget) yield a single whole-field
/// fragment with every match highlighted. Longer fields yield windowed fragments,
/// each anchored at an uncovered match and clamped to the char budget and to char
/// boundaries.
fn build_fragments(
    text: &str,
    matched: &[(usize, usize)],
    opts: &SnippetOptions,
) -> Vec<SnippetFragment> {
    if matched.is_empty() || opts.max_fragments == 0 || opts.max_fragment_chars == 0 {
        return Vec::new();
    }

    // Whole field fits in one fragment: emit it with all matches.
    if text.chars().count() <= opts.max_fragment_chars {
        let ranges = matched
            .iter()
            .map(|&(s, e)| HighlightRange { start: s, end: e })
            .collect();
        return vec![SnippetFragment {
            text: text.to_string(),
            ranges,
        }];
    }

    // Windowed fragments for longer fields.
    let mut fragments = Vec::new();
    let mut covered_until = 0usize; // byte offset already emitted in a fragment
    for &(ms, me) in matched {
        if ms < covered_until {
            continue; // already inside an emitted fragment
        }
        if fragments.len() >= opts.max_fragments {
            break;
        }
        let frag_start = ms;
        let frag_end = window_end(text, frag_start, opts.max_fragment_chars);
        let frag_text = text[frag_start..frag_end].to_string();
        // All matches fully inside this window become ranges (offsets are relative
        // to the fragment start).
        let ranges = matched
            .iter()
            .filter(|&&(s, e)| s >= frag_start && e <= frag_end)
            .map(|&(s, e)| HighlightRange {
                start: s - frag_start,
                end: e - frag_start,
            })
            .collect();
        let _ = me;
        fragments.push(SnippetFragment {
            text: frag_text,
            ranges,
        });
        covered_until = frag_end;
    }
    fragments
}

/// The byte offset `max_chars` characters after `start`, snapped to a char
/// boundary and clamped to the end of the string.
fn window_end(text: &str, start: usize, max_chars: usize) -> usize {
    text[start..]
        .char_indices()
        .nth(max_chars)
        .map(|(off, _)| start + off)
        .unwrap_or(text.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use auradb_index::AnalyzerPreset;

    fn fields(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn simple() -> Analyzer {
        Analyzer::new(AnalyzerPreset::Simple)
    }

    #[test]
    fn snippet_basic_match() {
        let f = fields(&[("body", "create, verify, and restore an AuraDB backup")]);
        let snip = build_snippet(
            "body",
            &["body"],
            &f,
            "create",
            simple(),
            &SnippetOptions::default(),
        )
        .expect("body is allowed and present");
        assert_eq!(snip.field, "body");
        assert_eq!(snip.fragments.len(), 1);
        let frag = &snip.fragments[0];
        assert_eq!(frag.text, "create, verify, and restore an AuraDB backup");
        assert_eq!(frag.ranges, vec![HighlightRange { start: 0, end: 6 }]);
        assert_eq!(&frag.text[0..6], "create");
    }

    #[test]
    fn snippet_field_allowlist() {
        let f = fields(&[("body", "hello world"), ("secret", "world")]);
        // Requesting a field outside the allowlist returns None — never read.
        assert!(build_snippet(
            "secret",
            &["body"],
            &f,
            "world",
            simple(),
            &SnippetOptions::default()
        )
        .is_none());
        // The allowed field still works.
        let snip = build_snippet(
            "body",
            &["body"],
            &f,
            "world",
            simple(),
            &SnippetOptions::default(),
        )
        .unwrap();
        assert_eq!(snip.field, "body");
        assert_eq!(snip.fragments.len(), 1);
    }

    #[test]
    fn snippet_no_hidden_fields() {
        // A document carrying an internal field must never surface it: the builder
        // only ever reads the requested allowed field's text.
        let f = fields(&[
            ("body", "the quick brown fox"),
            ("_internal", "TOP SECRET quick"),
            ("ssn", "123-45-6789 quick"),
        ]);
        let snip = build_snippet(
            "body",
            &["body"],
            &f,
            "quick",
            simple(),
            &SnippetOptions::default(),
        )
        .unwrap();
        let rendered = serde_json::to_string(&snip).unwrap();
        assert!(!rendered.contains("SECRET"));
        assert!(!rendered.contains("123-45-6789"));
        assert!(!rendered.contains("_internal"));
        assert!(rendered.contains("quick"));
    }

    #[test]
    fn snippet_fragment_limit() {
        // A long field with many matches must respect both caps.
        let body = (0..20)
            .map(|i| format!("alpha needle{i} beta gamma delta epsilon zeta"))
            .collect::<Vec<_>>()
            .join(" ");
        // Inject the matched term repeatedly far apart.
        let body = body.replace("alpha", "needle alpha");
        let f = fields(&[("body", body.as_str())]);
        let opts = SnippetOptions {
            max_fragments: 2,
            max_fragment_chars: 20,
        };
        let snip = build_snippet("body", &["body"], &f, "needle", simple(), &opts).unwrap();
        assert!(snip.fragments.len() <= 2, "must respect max_fragments");
        for frag in &snip.fragments {
            assert!(
                frag.text.chars().count() <= 20,
                "fragment exceeds char cap: {:?}",
                frag.text
            );
            assert!(!frag.ranges.is_empty());
        }
    }

    #[test]
    fn snippet_highlight_offsets() {
        let f = fields(&[("body", "restore the backup then verify the backup")]);
        let snip = build_snippet(
            "body",
            &["body"],
            &f,
            "backup",
            simple(),
            &SnippetOptions::default(),
        )
        .unwrap();
        let frag = &snip.fragments[0];
        // Two occurrences of "backup" highlighted; each range slices to "backup".
        assert_eq!(frag.ranges.len(), 2);
        for r in &frag.ranges {
            assert_eq!(&frag.text[r.start..r.end], "backup");
        }
    }

    #[test]
    fn snippet_unicode_offsets() {
        // Multibyte source: highlight offsets must land on char boundaries and
        // slice exactly the matched (original-cased) characters.
        let f = fields(&[("body", "le café est ouvert")]);
        let snip = build_snippet(
            "body",
            &["body"],
            &f,
            "café",
            Analyzer::new(AnalyzerPreset::Simple),
            &SnippetOptions::default(),
        )
        .unwrap();
        let frag = &snip.fragments[0];
        assert_eq!(frag.ranges.len(), 1);
        let r = frag.ranges[0];
        assert!(frag.text.is_char_boundary(r.start));
        assert!(frag.text.is_char_boundary(r.end));
        assert_eq!(&frag.text[r.start..r.end], "café");
    }

    #[test]
    fn snippet_ascii_fold_matches_accents() {
        // With the ascii_fold analyzer an unaccented query highlights accented text.
        let f = fields(&[("body", "le café est ouvert")]);
        let snip = build_snippet(
            "body",
            &["body"],
            &f,
            "cafe",
            Analyzer::new(AnalyzerPreset::AsciiFold),
            &SnippetOptions::default(),
        )
        .unwrap();
        assert_eq!(snip.fragments[0].ranges.len(), 1);
        let r = snip.fragments[0].ranges[0];
        assert_eq!(&snip.fragments[0].text[r.start..r.end], "café");
    }

    #[test]
    fn snippet_empty_query() {
        let f = fields(&[("body", "anything at all")]);
        let snip = build_snippet(
            "body",
            &["body"],
            &f,
            "",
            simple(),
            &SnippetOptions::default(),
        )
        .unwrap();
        // Present + allowed, but no match → empty fragments, not None.
        assert!(snip.fragments.is_empty());
    }

    #[test]
    fn snippet_no_match() {
        let f = fields(&[("body", "the quick brown fox")]);
        let snip = build_snippet(
            "body",
            &["body"],
            &f,
            "absent",
            simple(),
            &SnippetOptions::default(),
        )
        .unwrap();
        assert!(snip.fragments.is_empty());
    }
}
