//! Live (over-the-engine) query-time analyzer selection and search snippets.
//!
//! These exercise the v1.5.0 surface end-to-end through the embedded engine — the
//! same code path the server's `ReadRequest::Find` dispatch uses — proving that a
//! non-default analyzer changes retrieval and that opt-in snippets are produced,
//! field-allowlisted, and capped. The default analyzer must reproduce the v1.4
//! baseline exactly, and an absent analyzer/snippet request must behave as before.

use auradb::core::{CollectionSchema, Document, FieldDef, FieldType, IndexDef, IndexKind, Value};
use auradb::query::{
    FindQuery, FusionMode, HybridSearch, HybridWeights, SnippetRequest, TextOperator, TextRank,
    TextSearch,
};
use auradb::Engine;

fn schema() -> CollectionSchema {
    CollectionSchema::new("Doc")
        .with_field(FieldDef {
            name: "id".into(),
            field_type: FieldType::Uuid,
            primary_key: true,
            unique: true,
            nullable: false,
            indexed: false,
        })
        .with_field(FieldDef::new("body", FieldType::String))
        // A second text field used to prove the snippet allowlist and that a field
        // outside the request is never read.
        .with_field(FieldDef::new("secret", FieldType::String))
        // A non-text field: a snippet request naming it must be skipped, not panic.
        .with_field(FieldDef::new("views", FieldType::Int))
        .with_index(IndexDef {
            path: "body".into(),
            kind: IndexKind::FullText,
        })
}

fn doc(id: &str, body: &str, secret: &str, views: i64) -> Document {
    let mut f = Document::new();
    f.insert("id".into(), Value::Text(id.into()));
    f.insert("body".into(), Value::Text(body.into()));
    f.insert("secret".into(), Value::Text(secret.into()));
    f.insert("views".into(), Value::Int(views));
    f
}

fn row_id(row: &auradb::query::Row) -> String {
    match row.fields.get("id") {
        Some(Value::Text(s)) => s.clone(),
        _ => String::new(),
    }
}

fn text_search(query: &str, analyzer: Option<&str>) -> FindQuery {
    let mut q = FindQuery::new("Doc");
    q.text_search = Some(Box::new(TextSearch {
        field: "body".into(),
        query: query.into(),
        operator: TextOperator::Or,
        rank: TextRank::Bm25,
        k1: None,
        b: None,
        analyzer: analyzer.map(str::to_string),
    }));
    q
}

fn ids(engine: &Engine, q: &FindQuery) -> Vec<String> {
    let mut v: Vec<String> = engine.find(q).unwrap().iter().map(row_id).collect();
    v.sort();
    v
}

fn seed(engine: &Engine) {
    engine.create_schema(schema()).unwrap();
    engine
        .insert("Doc", doc("cafe", "le café est ouvert", "hidden one", 3))
        .unwrap();
    engine
        .insert("Doc", doc("naive", "a naïve approach", "hidden two", 9))
        .unwrap();
    engine
        .insert(
            "Doc",
            doc("plain", "ordinary coffee shop", "hidden three", 1),
        )
        .unwrap();
    engine
        .insert(
            "Doc",
            doc("backups", "verify the backups nightly", "hidden four", 5),
        )
        .unwrap();
}

#[test]
fn live_search_default_analyzer_matches_existing() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed(&engine);
    // An explicit `default` analyzer and an absent analyzer must return identical
    // results — the v1.4 baseline is preserved exactly.
    let absent = ids(&engine, &text_search("coffee", None));
    let explicit = ids(&engine, &text_search("coffee", Some("default")));
    assert_eq!(absent, explicit);
    assert_eq!(absent, vec!["plain".to_string()]);
}

#[test]
fn live_search_simple_analyzer_case_behavior() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed(&engine);
    // `simple` lowercases (like default); an upper-case query still matches.
    let lower = ids(&engine, &text_search("coffee", Some("simple")));
    let upper = ids(&engine, &text_search("COFFEE", Some("simple")));
    assert_eq!(lower, vec!["plain".to_string()]);
    assert_eq!(lower, upper);
}

#[test]
fn live_search_ascii_fold_matches_diacritic_fixture() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed(&engine);
    // Under `simple`, an unaccented query does NOT match the accented document.
    assert!(ids(&engine, &text_search("cafe", Some("simple"))).is_empty());
    assert!(ids(&engine, &text_search("naive", Some("simple"))).is_empty());
    // Under `ascii_fold`, it does — folding applies symmetrically to query + index.
    assert_eq!(
        ids(&engine, &text_search("cafe", Some("ascii_fold"))),
        vec!["cafe".to_string()]
    );
    assert_eq!(
        ids(&engine, &text_search("naive", Some("ascii_fold"))),
        vec!["naive".to_string()]
    );
}

#[test]
fn live_search_keyword_requires_whole_field_or_whole_term_behavior() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    engine.create_schema(schema()).unwrap();
    engine
        .insert("Doc", doc("a", "backup restore", "s", 1))
        .unwrap();
    engine
        .insert("Doc", doc("b", "backup and restore", "s", 1))
        .unwrap();
    engine
        .insert("Doc", doc("c", "restore the backup", "s", 1))
        .unwrap();
    // keyword matches the WHOLE field exactly (normalized), so only the document
    // whose entire field is "backup restore" matches — not the ones with extra
    // words or a different order.
    let got = ids(&engine, &text_search("backup restore", Some("keyword")));
    assert_eq!(got, vec!["a".to_string()]);
    // A whole-field query for a multi-word field that does not match anything.
    assert!(ids(&engine, &text_search("backup", Some("keyword"))).is_empty());
}

#[test]
fn live_search_english_basic_runs() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed(&engine);
    // The singular query "backup" matches the document containing "backups"
    // (conservative plural fold), and a stopword-only query returns nothing.
    assert_eq!(
        ids(&engine, &text_search("backup", Some("english_basic"))),
        vec!["backups".to_string()]
    );
    assert!(ids(&engine, &text_search("the", Some("english_basic"))).is_empty());
}

#[test]
fn live_search_english_basic_lens_regression() {
    // Regression for the bare-`s` singular fold: a document whose body contains
    // "lens" must be retrievable by the query "lens" under english_basic. Before the
    // `ns` protected-ending guard, both sides folded to "len" (still self-consistent,
    // but the wrong stem); now both sides keep "lens", which is the correct term.
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    engine.create_schema(schema()).unwrap();
    engine
        .insert("Doc", doc("optics", "clean the camera lens", "s", 1))
        .unwrap();
    engine
        .insert("Doc", doc("status", "current job status", "s", 1))
        .unwrap();
    // "lens" retrieves the optics doc and does not collapse to "len".
    assert_eq!(
        ids(&engine, &text_search("lens", Some("english_basic"))),
        vec!["optics".to_string()]
    );
    // A query for the truncated "len" must NOT match "lens" anymore.
    assert!(ids(&engine, &text_search("len", Some("english_basic"))).is_empty());
    // The `-us` singular "status" is likewise retrievable and unmangled.
    assert_eq!(
        ids(&engine, &text_search("status", Some("english_basic"))),
        vec!["status".to_string()]
    );
}

#[test]
fn live_search_unknown_analyzer_structured_error() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed(&engine);
    let err = engine
        .find(&text_search("coffee", Some("stemming")))
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("unknown analyzer"), "got: {msg}");
}

#[test]
fn live_search_explain_reports_analyzer() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed(&engine);
    let plan = engine
        .explain(&text_search("cafe", Some("ascii_fold")))
        .unwrap();
    let ts = plan.text_search.expect("ranked text plan present");
    assert_eq!(ts.analyzer, "ascii_fold");
    // The default path reports `default`.
    let plan = engine.explain(&text_search("coffee", None)).unwrap();
    assert_eq!(plan.text_search.unwrap().analyzer, "default");
}

#[test]
fn live_search_profile_reports_analyzer() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed(&engine);
    let plan = engine
        .explain_analyze(&text_search("cafe", Some("ascii_fold")))
        .unwrap();
    assert_eq!(plan.text_search.unwrap().analyzer, "ascii_fold");
    assert!(plan.analysis.is_some(), "explain analyze attaches metrics");
}

#[test]
fn old_query_without_analyzer_still_works() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed(&engine);
    // A query built with no analyzer field behaves exactly as in v1.4.
    let q = text_search("coffee", None);
    assert!(q.text_search.as_ref().unwrap().analyzer.is_none());
    assert_eq!(ids(&engine, &q), vec!["plain".to_string()]);
}

// --- snippets ---------------------------------------------------------------

fn snippet_query(query: &str, fields: &[&str], analyzer: Option<&str>) -> FindQuery {
    let mut q = text_search(query, analyzer);
    q.snippet = Some(SnippetRequest {
        fields: fields.iter().map(|s| s.to_string()).collect(),
        max_fragments: None,
        fragment_chars: None,
    });
    q
}

fn snippets_of(engine: &Engine, q: &FindQuery) -> Vec<auradb::query::Snippet> {
    let rows = engine.find(q).unwrap();
    rows.into_iter().flat_map(|r| r.snippets).collect()
}

#[test]
fn live_search_snippet_basic() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed(&engine);
    let snips = snippets_of(&engine, &snippet_query("coffee", &["body"], None));
    assert_eq!(snips.len(), 1);
    assert_eq!(snips[0].field, "body");
    assert_eq!(snips[0].fragments.len(), 1);
    let frag = &snips[0].fragments[0];
    assert!(frag.text.contains("coffee"));
    assert_eq!(frag.ranges.len(), 1);
    assert_eq!(
        &frag.text[frag.ranges[0].start..frag.ranges[0].end],
        "coffee"
    );
}

#[test]
fn live_search_snippet_opt_in_only() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed(&engine);
    // No snippet request -> no snippets on any row.
    for row in engine.find(&text_search("coffee", None)).unwrap() {
        assert!(row.snippets.is_empty());
    }
}

#[test]
fn live_search_snippet_field_allowlist_and_hidden_fields_not_returned() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    engine.create_schema(schema()).unwrap();
    engine
        .insert(
            "Doc",
            doc("a", "shared coffee word", "shared coffee word", 1),
        )
        .unwrap();
    // Only "body" is requested; "secret" contains the same query term but must
    // never be read or returned.
    let snips = snippets_of(&engine, &snippet_query("coffee", &["body"], None));
    assert_eq!(snips.len(), 1);
    assert!(snips.iter().all(|s| s.field == "body"));
    // Even serialized, the secret field name/text never appears via the snippet.
    let rows = engine
        .find(&snippet_query("coffee", &["body"], None))
        .unwrap();
    let json = serde_json::to_string(&rows).unwrap();
    // "secret" appears as a normal field value in `fields`, but never as a snippet
    // field; assert there is exactly one snippet and it is for body.
    let only_body = rows
        .iter()
        .flat_map(|r| &r.snippets)
        .all(|s| s.field == "body");
    assert!(
        only_body,
        "snippets must only cover allowlisted fields: {json}"
    );
}

#[test]
fn live_search_snippet_internal_field_never_eligible() {
    // A snippet request naming an internal (`_`-prefixed) field yields no snippet,
    // regardless of whether such a field exists.
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed(&engine);
    let snips = snippets_of(&engine, &snippet_query("coffee", &["_internal"], None));
    assert!(snips.is_empty());
}

#[test]
fn live_search_snippet_fragment_and_char_limits() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    engine.create_schema(schema()).unwrap();
    let body = (0..30)
        .map(|i| format!("filler{i} needle padding words here"))
        .collect::<Vec<_>>()
        .join(" ");
    engine.insert("Doc", doc("a", &body, "s", 1)).unwrap();
    let mut q = text_search("needle", None);
    q.snippet = Some(SnippetRequest {
        fields: vec!["body".into()],
        max_fragments: Some(2),
        fragment_chars: Some(20),
    });
    let snips = snippets_of(&engine, &q);
    assert_eq!(snips.len(), 1);
    assert!(snips[0].fragments.len() <= 2, "respects max_fragments");
    for frag in &snips[0].fragments {
        assert!(frag.text.chars().count() <= 20, "respects char cap");
    }
}

#[test]
fn live_search_snippet_unicode_safe() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed(&engine);
    // Multibyte source: the highlight range slices exactly the accented word, on
    // char boundaries (ascii_fold query against an accented field).
    let snips = snippets_of(
        &engine,
        &snippet_query("cafe", &["body"], Some("ascii_fold")),
    );
    assert_eq!(snips.len(), 1);
    let frag = &snips[0].fragments[0];
    let r = frag.ranges[0];
    assert!(frag.text.is_char_boundary(r.start) && frag.text.is_char_boundary(r.end));
    assert_eq!(&frag.text[r.start..r.end], "café");
}

#[test]
fn live_search_snippet_missing_and_non_text_field_safe() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed(&engine);
    // "missing" is not a field; "views" is an Int. Neither yields a snippet, and
    // neither panics. "body" still produces its snippet.
    let snips = snippets_of(
        &engine,
        &snippet_query("coffee", &["missing", "views", "body"], None),
    );
    assert_eq!(snips.len(), 1);
    assert_eq!(snips[0].field, "body");
}

#[test]
fn live_search_snippet_ranges_match_text() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    engine.create_schema(schema()).unwrap();
    engine
        .insert(
            "Doc",
            doc("a", "restore the backup then verify the backup", "s", 1),
        )
        .unwrap();
    let snips = snippets_of(&engine, &snippet_query("backup", &["body"], None));
    let frag = &snips[0].fragments[0];
    assert_eq!(frag.ranges.len(), 2);
    for r in &frag.ranges {
        assert_eq!(&frag.text[r.start..r.end], "backup");
    }
}

// --- hybrid + keyword analyzer ----------------------------------------------
//
// These prove the v1.5.0 fix: the `keyword` analyzer is accepted in hybrid search,
// the text component uses whole-field keyword semantics (no silent fallback to a
// per-token analyzer), the vector component still contributes, and EXPLAIN /
// EXPLAIN ANALYZE report `analyzer="keyword"` with a `keyword:` text source.

fn hybrid_schema() -> CollectionSchema {
    CollectionSchema::new("Hdoc")
        .with_field(FieldDef {
            name: "id".into(),
            field_type: FieldType::Uuid,
            primary_key: true,
            unique: true,
            nullable: false,
            indexed: false,
        })
        .with_field(FieldDef::new("body", FieldType::String))
        .with_field(FieldDef::new("embedding", FieldType::Vector { dim: 3 }))
        .with_index(IndexDef {
            path: "body".into(),
            kind: IndexKind::FullText,
        })
}

fn hdoc(id: &str, body: &str, vec: Vec<f32>) -> Document {
    let mut f = Document::new();
    f.insert("id".into(), Value::Text(id.into()));
    f.insert("body".into(), Value::Text(body.into()));
    f.insert("embedding".into(), Value::Vector(vec));
    f
}

fn hybrid_query(
    text: &str,
    vector: Vec<f32>,
    analyzer: Option<&str>,
    weights: HybridWeights,
    fusion: FusionMode,
) -> FindQuery {
    let mut q = FindQuery::new("Hdoc");
    q.hybrid = Some(Box::new(HybridSearch {
        text_field: "body".into(),
        text_query: text.into(),
        vector_field: "embedding".into(),
        vector,
        top_k: 10,
        metric: None,
        weights,
        fusion,
        operator: TextOperator::Or,
        k1: None,
        b: None,
        analyzer: analyzer.map(str::to_string),
    }));
    q
}

fn seed_hybrid_keyword(engine: &Engine) {
    engine.create_schema(hybrid_schema()).unwrap();
    // "exact" — whole field normalizes to the keyword query "backup restore".
    engine
        .insert("Hdoc", hdoc("exact", "Backup Restore", vec![1.0, 0.0, 0.0]))
        .unwrap();
    // "partial" — contains the same tokens plus extra words; keyword must NOT match
    // it (whole-field semantics), but its vector is irrelevant here.
    engine
        .insert(
            "Hdoc",
            hdoc("partial", "backup and restore now", vec![0.0, 1.0, 0.0]),
        )
        .unwrap();
    // "vector_only" — body shares no keyword with the query, but its vector is the
    // exact query vector, so the vector signal must still surface it.
    engine
        .insert(
            "Hdoc",
            hdoc("vector_only", "totally unrelated text", vec![0.0, 0.0, 1.0]),
        )
        .unwrap();
}

fn hrow_id(row: &auradb::query::Row) -> String {
    match row.fields.get("id") {
        Some(Value::Text(s)) => s.clone(),
        _ => String::new(),
    }
}

#[test]
fn hybrid_keyword_analyzer_accepts_query() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed_hybrid_keyword(&engine);
    // The whole point of the fix: this no longer errors.
    let rows = engine
        .find(&hybrid_query(
            "backup restore",
            vec![1.0, 0.0, 0.0],
            Some("keyword"),
            HybridWeights::default(),
            FusionMode::WeightedSum,
        ))
        .expect("keyword analyzer must be accepted in hybrid search");
    assert!(!rows.is_empty());
    assert!(rows.iter().any(|r| hrow_id(r) == "exact"));
}

#[test]
fn hybrid_keyword_analyzer_text_component_matches_keyword_fixture() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed_hybrid_keyword(&engine);
    let rows = engine
        .find(&hybrid_query(
            "backup restore",
            vec![1.0, 0.0, 0.0],
            Some("keyword"),
            // Text-only weighting isolates the keyword text component.
            HybridWeights {
                text: 1.0,
                vector: 0.0,
            },
            FusionMode::WeightedSum,
        ))
        .unwrap();
    // Only "exact" gets a text score: keyword is whole-field, so "partial" (extra
    // words) does not match the text signal.
    let exact = rows.iter().find(|r| hrow_id(r) == "exact").unwrap();
    assert!(
        exact.text_score.is_some(),
        "exact whole-field keyword match"
    );
    let partial = rows.iter().find(|r| hrow_id(r) == "partial");
    assert!(
        partial.map(|r| r.text_score).unwrap_or(None).is_none(),
        "partial field must not match keyword text component"
    );
}

#[test]
fn hybrid_keyword_analyzer_vector_component_still_contributes() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed_hybrid_keyword(&engine);
    // Query vector points exactly at "vector_only", which shares no keyword term.
    let rows = engine
        .find(&hybrid_query(
            "backup restore",
            vec![0.0, 0.0, 1.0],
            Some("keyword"),
            HybridWeights::default(),
            FusionMode::WeightedSum,
        ))
        .unwrap();
    let vector_only = rows
        .iter()
        .find(|r| hrow_id(r) == "vector_only")
        .expect("vector signal must still contribute candidates under keyword");
    assert!(vector_only.vector_score.is_some());
    assert!(
        vector_only.text_score.is_none(),
        "vector_only has no keyword text match"
    );
}

#[test]
fn hybrid_keyword_analyzer_reports_in_explain() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed_hybrid_keyword(&engine);
    let plan = engine
        .explain(&hybrid_query(
            "backup restore",
            vec![1.0, 0.0, 0.0],
            Some("keyword"),
            HybridWeights::default(),
            FusionMode::WeightedSum,
        ))
        .unwrap();
    let h = plan.hybrid.expect("hybrid plan present");
    assert_eq!(h.analyzer, "keyword");
    // No silent fallback: the text source names the keyword path, not bm25.
    assert_eq!(h.text_source, "keyword:body");
}

#[test]
fn hybrid_keyword_analyzer_reports_in_profile() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed_hybrid_keyword(&engine);
    let plan = engine
        .explain_analyze(&hybrid_query(
            "backup restore",
            vec![1.0, 0.0, 0.0],
            Some("keyword"),
            HybridWeights::default(),
            FusionMode::WeightedSum,
        ))
        .unwrap();
    let h = plan.hybrid.expect("hybrid plan present");
    assert_eq!(h.analyzer, "keyword");
    assert_eq!(h.text_source, "keyword:body");
    assert!(plan.analysis.is_some(), "explain analyze attaches metrics");
}

#[test]
fn hybrid_keyword_analyzer_unknown_still_errors() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed_hybrid_keyword(&engine);
    let err = engine
        .find(&hybrid_query(
            "backup restore",
            vec![1.0, 0.0, 0.0],
            Some("stemming"),
            HybridWeights::default(),
            FusionMode::WeightedSum,
        ))
        .unwrap_err();
    assert!(err.to_string().contains("unknown analyzer"), "got: {err}");
}

#[test]
fn hybrid_default_behavior_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed_hybrid_keyword(&engine);
    // An absent analyzer and an explicit `default` analyzer produce identical fused
    // results — the v1.4 hybrid baseline is preserved exactly.
    let absent: Vec<String> = engine
        .find(&hybrid_query(
            "backup",
            vec![1.0, 0.0, 0.0],
            None,
            HybridWeights::default(),
            FusionMode::WeightedSum,
        ))
        .unwrap()
        .iter()
        .map(hrow_id)
        .collect();
    let explicit: Vec<String> = engine
        .find(&hybrid_query(
            "backup",
            vec![1.0, 0.0, 0.0],
            Some("default"),
            HybridWeights::default(),
            FusionMode::WeightedSum,
        ))
        .unwrap()
        .iter()
        .map(hrow_id)
        .collect();
    assert_eq!(absent, explicit);
    // The default hybrid EXPLAIN still reports the bm25 text source.
    let plan = engine
        .explain(&hybrid_query(
            "backup",
            vec![1.0, 0.0, 0.0],
            None,
            HybridWeights::default(),
            FusionMode::WeightedSum,
        ))
        .unwrap();
    assert_eq!(plan.hybrid.unwrap().text_source, "bm25:body");
}

#[test]
fn hybrid_keyword_conformance_path() {
    // The end-to-end shape the connector conformance suite drives: a keyword-hybrid
    // query returns the exact whole-field match as a fully fused row (both component
    // scores and a rank present) under both fusion modes.
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(dir.path()).unwrap();
    seed_hybrid_keyword(&engine);
    for fusion in [FusionMode::WeightedSum, FusionMode::ReciprocalRankFusion] {
        let rows = engine
            .find(&hybrid_query(
                "backup restore",
                vec![1.0, 0.0, 0.0],
                Some("keyword"),
                HybridWeights::default(),
                fusion,
            ))
            .unwrap();
        let exact = rows
            .iter()
            .find(|r| hrow_id(r) == "exact")
            .expect("exact keyword match present in fused results");
        assert!(exact.text_score.is_some());
        assert!(exact.vector_score.is_some());
        assert!(exact.score.is_some());
        assert!(exact.rank.is_some());
    }
}
