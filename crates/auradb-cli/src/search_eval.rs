//! `auradb search eval` — a deterministic, offline search-relevance evaluation
//! harness.
//!
//! Given a relevance dataset (a JSONL corpus, a JSONL query set, and JSONL
//! relevance judgments), this ingests the corpus into a fresh data directory,
//! runs each query through one of the engine's already-implemented ranked
//! retrieval paths (BM25 text, exact vector, or hybrid fusion), and reports
//! MRR@k, NDCG@k, and Recall@k as a machine-readable JSON report.
//!
//! The numbers are *dataset-specific*: they describe how the ranker ordered the
//! fixture documents for the fixture queries. They are a regression signal, not a
//! universal benchmark of search quality, and they make no ANN or HA claim — the
//! vector path used here is the exact baseline.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{bail, Context, Result};
use auradb::core::{CollectionSchema, Document, FieldDef, FieldType, IndexDef, IndexKind, Value};
use auradb::query::{
    mrr_at_k, ndcg_at_k, recall_at_k, relevant_set, FindQuery, FusionMode, HybridSearch,
    HybridWeights, Row, TextOperator, TextRank, TextSearch, VectorSearch, BM25_DEFAULT_B,
    BM25_DEFAULT_K1,
};
use auradb::Engine;
use serde::{Deserialize, Serialize};

/// The collection the harness ingests the corpus into.
const COLLECTION: &str = "RelevanceDoc";
/// The full-text indexed field the corpus text is concatenated into.
const TEXT_FIELD: &str = "text";
/// The exact-vector field used by the vector and hybrid modes.
const VECTOR_FIELD: &str = "embedding";

/// A retrieval mode the harness can evaluate. Each maps to an already-implemented
/// engine retrieval path; the harness adds no new ranking behaviour of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EvalMode {
    /// BM25 ranked full-text search over the concatenated text field.
    Bm25,
    /// Exact (brute-force) vector nearest-neighbour search — the correctness
    /// baseline, never the approximate preview.
    VectorExact,
    /// Hybrid fusion of the BM25 text signal and the exact vector signal.
    Hybrid,
}

impl EvalMode {
    fn parse(s: &str) -> Result<Self> {
        match s {
            "bm25" => Ok(EvalMode::Bm25),
            "vector_exact" => Ok(EvalMode::VectorExact),
            "hybrid" => Ok(EvalMode::Hybrid),
            other => bail!("unknown --mode {other:?}; expected bm25, vector_exact, or hybrid"),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            EvalMode::Bm25 => "bm25",
            EvalMode::VectorExact => "vector_exact",
            EvalMode::Hybrid => "hybrid",
        }
    }

    fn uses_text(self) -> bool {
        matches!(self, EvalMode::Bm25 | EvalMode::Hybrid)
    }

    fn uses_vector(self) -> bool {
        matches!(self, EvalMode::VectorExact | EvalMode::Hybrid)
    }
}

/// One corpus document as read from the JSONL corpus file.
#[derive(Debug, Clone, Deserialize)]
struct CorpusDoc {
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    category: String,
    #[serde(default)]
    vector: Option<Vec<f32>>,
}

/// One evaluation query as read from the JSONL queries file.
#[derive(Debug, Clone, Deserialize)]
struct QueryRow {
    id: String,
    text: String,
    #[serde(default)]
    #[allow(dead_code)]
    tags: Vec<String>,
    #[serde(default)]
    vector: Option<Vec<f32>>,
}

/// One relevance judgment (qrel) as read from the JSONL qrels file.
#[derive(Debug, Clone, Deserialize)]
struct Qrel {
    query_id: String,
    doc_id: String,
    relevance: i64,
}

/// BM25 parameters echoed into the report so a tuning run records exactly what it
/// measured.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Bm25Params {
    /// Term-saturation parameter.
    pub k1: f32,
    /// Length-normalization parameter.
    pub b: f32,
}

/// Hybrid fusion weights echoed into the report.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct WeightsReport {
    /// Weight applied to the text relevance signal.
    pub text: f32,
    /// Weight applied to the vector similarity signal.
    pub vector: f32,
}

/// The aggregate (mean across evaluated queries) relevance metrics.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct AggregateMetrics {
    /// Mean reciprocal rank at `k`.
    pub mrr_at_k: f64,
    /// Mean normalized discounted cumulative gain at `k`.
    pub ndcg_at_k: f64,
    /// Mean recall at `k`.
    pub recall_at_k: f64,
}

/// Per-query metrics plus the ranked document ids the query returned.
#[derive(Debug, Clone, Serialize)]
pub struct PerQueryMetrics {
    /// The query id from the dataset.
    pub query_id: String,
    /// Reciprocal rank at `k` for this query.
    pub mrr_at_k: f64,
    /// NDCG at `k` for this query.
    pub ndcg_at_k: f64,
    /// Recall at `k` for this query.
    pub recall_at_k: f64,
    /// The ranked document ids this query returned (best first, up to `k`).
    pub top_docs: Vec<String>,
}

/// The machine-readable search-relevance report emitted by `auradb search eval`.
///
/// Every metric is measured on the supplied dataset with the engine's existing
/// retrieval paths. The numbers are dataset-specific and are not a universal
/// search-quality benchmark.
#[derive(Debug, Clone, Serialize)]
pub struct SearchEvalReport {
    /// The dataset label (derived from the corpus file name).
    pub dataset: String,
    /// The evaluated retrieval mode (`bm25`, `vector_exact`, or `hybrid`).
    pub mode: String,
    /// The number of queries evaluated.
    pub queries: usize,
    /// The number of corpus documents ingested.
    pub documents: usize,
    /// The rank cutoff `k`.
    pub k: usize,
    /// Which BM25 preset produced the parameters (`default` or `custom`), present
    /// for the text-bearing modes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// The effective BM25 parameters, present for the text-bearing modes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bm25: Option<Bm25Params>,
    /// The hybrid fusion weights, present only for hybrid mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weights: Option<WeightsReport>,
    /// The aggregate metrics across the evaluated queries.
    pub metrics: AggregateMetrics,
    /// The per-query metrics, in dataset order.
    pub per_query: Vec<PerQueryMetrics>,
    /// Honest warnings (queries without judgments, missing query vectors, qrels
    /// referencing unknown ids).
    pub warnings: Vec<String>,
}

fn read_jsonl<T: for<'de> Deserialize<'de>>(path: &Path, what: &str) -> Result<Vec<T>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading {what} file {}", path.display()))?;
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let row: T = serde_json::from_str(trimmed)
            .with_context(|| format!("parsing {what} line {} as JSON", i + 1))?;
        out.push(row);
    }
    if out.is_empty() {
        bail!("{what} file {} contained no records", path.display());
    }
    Ok(out)
}

fn parse_corpus(path: &Path) -> Result<Vec<CorpusDoc>> {
    let docs: Vec<CorpusDoc> = read_jsonl(path, "corpus")?;
    let mut seen = HashSet::new();
    for d in &docs {
        if d.id.trim().is_empty() {
            bail!("corpus contains a document with an empty id");
        }
        if !seen.insert(d.id.clone()) {
            bail!("corpus contains a duplicate document id {:?}", d.id);
        }
    }
    Ok(docs)
}

fn parse_queries(path: &Path) -> Result<Vec<QueryRow>> {
    let queries: Vec<QueryRow> = read_jsonl(path, "queries")?;
    let mut seen = HashSet::new();
    for q in &queries {
        if q.id.trim().is_empty() {
            bail!("queries contain a query with an empty id");
        }
        if q.text.trim().is_empty() {
            bail!("query {:?} has empty text", q.id);
        }
        if !seen.insert(q.id.clone()) {
            bail!("queries contain a duplicate query id {:?}", q.id);
        }
    }
    Ok(queries)
}

fn parse_qrels(path: &Path) -> Result<Vec<Qrel>> {
    let qrels: Vec<Qrel> = read_jsonl(path, "qrels")?;
    for r in &qrels {
        if r.query_id.trim().is_empty() || r.doc_id.trim().is_empty() {
            bail!("qrels contain a judgment with an empty query_id or doc_id");
        }
        if r.relevance < 0 {
            bail!(
                "qrels contain a negative relevance grade {} for ({:?}, {:?}); grades must be >= 0",
                r.relevance,
                r.query_id,
                r.doc_id
            );
        }
    }
    Ok(qrels)
}

/// The concatenated, full-text-indexed searchable text for one corpus document:
/// title, body, tags, and category joined with spaces. Documented in the fixture
/// README so dataset authors know what BM25 sees.
fn searchable_text(doc: &CorpusDoc) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if !doc.title.is_empty() {
        parts.push(&doc.title);
    }
    if !doc.body.is_empty() {
        parts.push(&doc.body);
    }
    let tags = doc.tags.join(" ");
    if !tags.is_empty() {
        parts.push(&tags);
    }
    if !doc.category.is_empty() {
        parts.push(&doc.category);
    }
    parts.join(" ")
}

/// Validate the corpus vectors, returning the common dimensionality when every
/// document carries a vector of the same length. Returns `Ok(None)` when no
/// document has a vector.
fn corpus_vector_dim(docs: &[CorpusDoc]) -> Result<Option<usize>> {
    let mut dim: Option<usize> = None;
    let mut with_vec = 0usize;
    for d in docs {
        if let Some(v) = &d.vector {
            with_vec += 1;
            match dim {
                None => dim = Some(v.len()),
                Some(expected) if expected != v.len() => bail!(
                    "corpus document {:?} has vector length {} but {} was expected",
                    d.id,
                    v.len(),
                    expected
                ),
                Some(_) => {}
            }
        }
    }
    if with_vec == 0 {
        return Ok(None);
    }
    if with_vec != docs.len() {
        bail!("corpus mixes documents with and without vectors; either all or none must have a vector");
    }
    Ok(dim)
}

fn ingest(engine: &Engine, docs: &[CorpusDoc], vector_dim: Option<usize>) -> Result<()> {
    let mut schema = CollectionSchema::new(COLLECTION)
        .with_field(FieldDef {
            name: "id".into(),
            field_type: FieldType::Uuid,
            primary_key: true,
            unique: true,
            nullable: false,
            indexed: false,
        })
        .with_field(FieldDef::new(TEXT_FIELD, FieldType::String))
        .with_index(IndexDef {
            path: TEXT_FIELD.into(),
            kind: IndexKind::FullText,
        });
    if let Some(dim) = vector_dim {
        schema = schema.with_field(FieldDef::new(VECTOR_FIELD, FieldType::Vector { dim }));
    }
    engine
        .create_schema(schema)
        .context("creating the relevance-evaluation schema (the data dir must be empty)")?;

    for d in docs {
        let mut fields = Document::new();
        fields.insert("id".into(), Value::Text(d.id.clone()));
        fields.insert(TEXT_FIELD.into(), Value::Text(searchable_text(d)));
        if let Some(v) = &d.vector {
            fields.insert(VECTOR_FIELD.into(), Value::Vector(v.clone()));
        }
        engine
            .insert(COLLECTION, fields)
            .with_context(|| format!("inserting corpus document {:?}", d.id))?;
    }
    engine.analyze().context("collecting planner statistics")?;
    Ok(())
}

/// Extract the dataset document ids from a ranked result set, preserving order.
fn ranked_ids(rows: &[Row]) -> Vec<String> {
    rows.iter()
        .filter_map(|r| match r.fields.get("id") {
            Some(Value::Text(s)) => Some(s.clone()),
            _ => None,
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn build_query(
    mode: EvalMode,
    query: &QueryRow,
    k: usize,
    k1: Option<f32>,
    b: Option<f32>,
    weights: WeightsReport,
) -> Option<FindQuery> {
    let mut q = FindQuery::new(COLLECTION);
    q.limit = Some(k);
    match mode {
        EvalMode::Bm25 => {
            q.text_search = Some(Box::new(TextSearch {
                field: TEXT_FIELD.into(),
                query: query.text.clone(),
                operator: TextOperator::Or,
                rank: TextRank::Bm25,
                k1,
                b,
            }));
        }
        EvalMode::VectorExact => {
            let vector = query.vector.clone()?;
            q.vector = Some(VectorSearch {
                field: VECTOR_FIELD.into(),
                query: vector,
                k,
                metric: "cosine".into(),
            });
        }
        EvalMode::Hybrid => {
            let vector = query.vector.clone()?;
            q.hybrid = Some(Box::new(HybridSearch {
                text_field: TEXT_FIELD.into(),
                text_query: query.text.clone(),
                vector_field: VECTOR_FIELD.into(),
                vector,
                top_k: k,
                metric: Some("cosine".into()),
                weights: HybridWeights {
                    text: weights.text,
                    vector: weights.vector,
                },
                fusion: FusionMode::WeightedSum,
                operator: TextOperator::Or,
                k1,
                b,
            }));
        }
    }
    Some(q)
}

/// `auradb search eval` — evaluate ranked-retrieval relevance for a dataset.
///
/// Ingests `corpus` into a fresh `data_dir`, runs each query from `queries`
/// through the requested `mode`, scores the results against `qrels`, and returns
/// the report as pretty-printed JSON. `data_dir` must be empty (the harness owns
/// the collection it creates). Returns an error for malformed datasets, unknown
/// modes, or invalid BM25/weight parameters, so callers (and CI) get a non-zero
/// exit on a bad dataset.
#[allow(clippy::too_many_arguments)]
pub fn cmd_search_eval(
    data_dir: &Path,
    corpus: &Path,
    queries: &Path,
    qrels: &Path,
    mode: &str,
    k: usize,
    k1: Option<f32>,
    b: Option<f32>,
    text_weight: f32,
    vector_weight: f32,
) -> Result<String> {
    if k == 0 {
        bail!("k must be >= 1");
    }
    let mode = EvalMode::parse(mode)?;

    // Validate BM25 parameter overrides up front (defaults preserve current
    // behaviour: None means the engine's built-in BM25_DEFAULT_K1 / _B).
    if let Some(k1) = k1 {
        if !(k1.is_finite() && k1 >= 0.0) {
            bail!("--k1 must be finite and >= 0");
        }
    }
    if let Some(b) = b {
        if !(b.is_finite() && (0.0..=1.0).contains(&b)) {
            bail!("--b must be in [0, 1]");
        }
    }
    // Validate hybrid weights even when unused so a bad value is rejected loudly.
    if !(text_weight.is_finite() && vector_weight.is_finite())
        || text_weight < 0.0
        || vector_weight < 0.0
        || (text_weight == 0.0 && vector_weight == 0.0)
    {
        bail!("--text-weight and --vector-weight must be finite, >= 0, and not both zero");
    }
    let weights = WeightsReport {
        text: text_weight,
        vector: vector_weight,
    };

    let docs = parse_corpus(corpus)?;
    let query_rows = parse_queries(queries)?;
    let judgments = parse_qrels(qrels)?;

    let vector_dim = corpus_vector_dim(&docs)?;
    if mode.uses_vector() && vector_dim.is_none() {
        bail!(
            "mode {} requires vectors but the corpus has none; add a `vector` field or use --mode bm25",
            mode.as_str()
        );
    }

    // Index the dataset ids so we can warn about qrels that reference unknown
    // documents or queries.
    let doc_ids: HashSet<&str> = docs.iter().map(|d| d.id.as_str()).collect();
    let query_ids: HashSet<&str> = query_rows.iter().map(|q| q.id.as_str()).collect();

    // grades[query_id][doc_id] = grade.
    let mut grades: HashMap<String, HashMap<String, u32>> = HashMap::new();
    let mut warnings: Vec<String> = Vec::new();
    for r in &judgments {
        if !query_ids.contains(r.query_id.as_str()) {
            warnings.push(format!(
                "qrel references unknown query_id {:?}; ignored",
                r.query_id
            ));
            continue;
        }
        if !doc_ids.contains(r.doc_id.as_str()) {
            warnings.push(format!(
                "qrel references unknown doc_id {:?}; ignored",
                r.doc_id
            ));
            continue;
        }
        grades
            .entry(r.query_id.clone())
            .or_default()
            .insert(r.doc_id.clone(), r.relevance as u32);
    }

    let engine = Engine::open(data_dir)?;
    ingest(&engine, &docs, vector_dim)?;

    let mut per_query: Vec<PerQueryMetrics> = Vec::with_capacity(query_rows.len());
    let mut sum_mrr = 0.0;
    let mut sum_ndcg = 0.0;
    let mut sum_recall = 0.0;
    let mut scored_queries = 0usize;

    for query in &query_rows {
        let query_grades = grades.get(&query.id).cloned().unwrap_or_default();
        let relevant = relevant_set(&query_grades);

        let top_docs = match build_query(mode, query, k, k1, b, weights) {
            Some(q) => {
                let rows = engine
                    .find(&q)
                    .with_context(|| format!("evaluating query {:?}", query.id))?;
                ranked_ids(&rows)
            }
            None => {
                warnings.push(format!(
                    "query {:?} has no vector; skipped in mode {}",
                    query.id,
                    mode.as_str()
                ));
                Vec::new()
            }
        };

        let mrr = mrr_at_k(&top_docs, &relevant, k);
        let ndcg = ndcg_at_k(&top_docs, &query_grades, k);
        let recall = recall_at_k(&top_docs, &relevant, k);

        if relevant.is_empty() {
            warnings.push(format!(
                "query {:?} has no relevant judgments; excluded from aggregate metrics",
                query.id
            ));
        } else {
            sum_mrr += mrr;
            sum_ndcg += ndcg;
            sum_recall += recall;
            scored_queries += 1;
        }

        per_query.push(PerQueryMetrics {
            query_id: query.id.clone(),
            mrr_at_k: mrr,
            ndcg_at_k: ndcg,
            recall_at_k: recall,
            top_docs,
        });
    }

    let denom = scored_queries.max(1) as f64;
    let metrics = AggregateMetrics {
        mrr_at_k: sum_mrr / denom,
        ndcg_at_k: sum_ndcg / denom,
        recall_at_k: sum_recall / denom,
    };
    if scored_queries == 0 {
        warnings.push("no query had relevant judgments; aggregate metrics are zero".into());
    }

    let (preset, bm25) = if mode.uses_text() {
        let effective = Bm25Params {
            k1: k1.unwrap_or(BM25_DEFAULT_K1),
            b: b.unwrap_or(BM25_DEFAULT_B),
        };
        let preset = if k1.is_none() && b.is_none() {
            "default"
        } else {
            "custom"
        };
        (Some(preset.to_string()), Some(effective))
    } else {
        (None, None)
    };
    let weights_report = if mode == EvalMode::Hybrid {
        Some(weights)
    } else {
        None
    };

    let report = SearchEvalReport {
        dataset: dataset_label(corpus),
        mode: mode.as_str().to_string(),
        queries: query_rows.len(),
        documents: docs.len(),
        k,
        preset,
        bm25,
        weights: weights_report,
        metrics,
        per_query,
        warnings,
    };
    Ok(serde_json::to_string_pretty(&report)?)
}

/// Derive a short dataset label from the corpus file name, stripping a trailing
/// `_corpus` so `small_corpus.jsonl` becomes `small`.
fn dataset_label(corpus: &Path) -> String {
    let stem = corpus
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("dataset");
    stem.strip_suffix("_corpus").unwrap_or(stem).to_string()
}
