//! Deterministic query-time analyzers and tokenizer presets.
//!
//! An [`Analyzer`] turns an input string into an ordered [`TokenStream`] of
//! [`Token`]s. Every token carries byte offsets into the *original* input, so the
//! same machinery can drive both retrieval (via [`Analyzer::index_terms`]) and a
//! future highlight/snippet layer (via the offsets on [`analyze`](Analyzer::analyze)).
//!
//! The presets are intentionally small and dependency-free:
//!
//! * [`AnalyzerPreset::Default`] — the v1.x engine tokenizer, given a name. It
//!   lowercases and splits on every non-alphanumeric boundary. Selecting it changes
//!   nothing, so existing search behavior is preserved exactly. It is identical to
//!   [`crate::tokenize`].
//! * [`AnalyzerPreset::Simple`] — the same tokenization as `Default`, exposed under
//!   an explicit name for callers who want to be explicit. On any input it emits the
//!   same terms as `Default`.
//! * [`AnalyzerPreset::AsciiFold`] — `Simple` plus a fixed ASCII-folding table for
//!   common Latin diacritics (for example `café` → `cafe`). No external dictionary.
//! * [`AnalyzerPreset::Keyword`] — the whole input as a single token (trimmed and
//!   lowercased), for exact whole-field matching of short fields.
//! * [`AnalyzerPreset::EnglishBasic`] — `Simple` tokenization plus a small built-in
//!   English stopword list and a conservative, deterministic plural fold (for
//!   example `backups` → `backup`, `boxes` → `box`, `policies` → `policy`). It is a
//!   tiny built-in helper, **not** a stemmer and **not** full language-aware NLP:
//!   there is no dictionary, no `-ing`/`-ed` handling, and no part-of-speech model.
//!
//! Determinism is a hard requirement: for a given preset and input the token order,
//! token text, and offsets are fixed. Every preset except `english_basic` applies no
//! stopword removal; `english_basic` removes a small fixed list and folds a narrow,
//! tested set of plural suffixes. No preset uses a language model or external
//! dictionary, so the analyzers make no language claims beyond the mechanical
//! transformations documented above.

use auradb_core::{Error, Result};

/// A single analyzed token with byte offsets into the original input.
///
/// `start..end` is a byte range into the string that was analyzed. For folding or
/// keyword presets the [`text`](Token::text) may differ from `input[start..end]`
/// (it is lowercased and/or ASCII-folded), but the offsets always point at the
/// untouched source span — which is what a highlighter needs to underline the
/// original characters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// The normalized token text the analyzer emits (lowercased, possibly folded).
    pub text: String,
    /// Inclusive byte offset into the original input where the source span begins.
    pub start: usize,
    /// Exclusive byte offset into the original input where the source span ends.
    pub end: usize,
}

/// The ordered tokens produced by analyzing one input, in left-to-right source
/// order. Offsets index the original (pre-analysis) input.
pub type TokenStream = Vec<Token>;

/// A built-in, deterministic analyzer preset. New presets are additive; the set is
/// closed and every variant is dependency-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AnalyzerPreset {
    /// The v1.x engine tokenizer: lowercase + split on non-alphanumeric boundaries.
    /// Identical to [`crate::tokenize`]; the default so existing behavior is kept.
    #[default]
    Default,
    /// The same tokenization as [`AnalyzerPreset::Default`], under an explicit name.
    Simple,
    /// [`AnalyzerPreset::Simple`] plus ASCII-folding of common Latin diacritics.
    AsciiFold,
    /// The entire input as one trimmed, lowercased token (exact whole-field match).
    Keyword,
    /// [`AnalyzerPreset::Simple`] plus a small built-in English stopword list and a
    /// conservative plural fold. A tiny built-in helper, not a stemmer or full NLP.
    EnglishBasic,
}

impl AnalyzerPreset {
    /// Every built-in preset, in a stable order (used by tooling that enumerates
    /// the available analyzers, e.g. `search eval compare-analyzers`).
    pub const ALL: [AnalyzerPreset; 5] = [
        AnalyzerPreset::Default,
        AnalyzerPreset::Simple,
        AnalyzerPreset::AsciiFold,
        AnalyzerPreset::Keyword,
        AnalyzerPreset::EnglishBasic,
    ];

    /// The stable wire/CLI name of this preset.
    pub fn name(self) -> &'static str {
        match self {
            AnalyzerPreset::Default => "default",
            AnalyzerPreset::Simple => "simple",
            AnalyzerPreset::AsciiFold => "ascii_fold",
            AnalyzerPreset::Keyword => "keyword",
            AnalyzerPreset::EnglishBasic => "english_basic",
        }
    }

    /// Parse a preset by name. An unknown name yields a structured
    /// [`Error::InvalidRequest`] listing the supported presets, so callers can
    /// surface a clear error rather than silently falling back to a default.
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "default" => Ok(AnalyzerPreset::Default),
            "simple" => Ok(AnalyzerPreset::Simple),
            "ascii_fold" => Ok(AnalyzerPreset::AsciiFold),
            "keyword" => Ok(AnalyzerPreset::Keyword),
            "english_basic" => Ok(AnalyzerPreset::EnglishBasic),
            other => Err(Error::InvalidRequest(format!(
                "unknown analyzer {other:?}; expected one of: \
                 default, simple, ascii_fold, keyword, english_basic"
            ))),
        }
    }

    /// Whether this preset is a *per-token* transform of the [`AnalyzerPreset::Default`]
    /// tokenization — i.e. each default token maps independently to zero or more
    /// output terms. Every preset except [`AnalyzerPreset::Keyword`] (which collapses
    /// the whole field into one term) is per-token.
    ///
    /// This is the property the live search engine relies on to evaluate an analyzer
    /// over the persisted default-tokenized postings without re-indexing: see
    /// [`Analyzer::map_default_token`].
    pub fn is_per_token(self) -> bool {
        !matches!(self, AnalyzerPreset::Keyword)
    }
}

/// A configured analyzer. Cheap to construct and copy; holds only its preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Analyzer {
    preset: AnalyzerPreset,
}

impl Analyzer {
    /// Construct an analyzer for a preset.
    pub fn new(preset: AnalyzerPreset) -> Self {
        Analyzer { preset }
    }

    /// Construct an analyzer from a preset name (see [`AnalyzerPreset::parse`]).
    pub fn parse(name: &str) -> Result<Self> {
        Ok(Analyzer::new(AnalyzerPreset::parse(name)?))
    }

    /// The preset this analyzer applies.
    pub fn preset(self) -> AnalyzerPreset {
        self.preset
    }

    /// The stable name of this analyzer's preset.
    pub fn name(self) -> &'static str {
        self.preset.name()
    }

    /// Analyze `text` into an ordered [`TokenStream`] with byte offsets into the
    /// original input. Deterministic for a given preset and input.
    pub fn analyze(self, text: &str) -> TokenStream {
        match self.preset {
            AnalyzerPreset::Default | AnalyzerPreset::Simple => split_alphanumeric(text, false),
            AnalyzerPreset::AsciiFold => split_alphanumeric(text, true),
            AnalyzerPreset::Keyword => keyword_token(text).into_iter().collect(),
            AnalyzerPreset::EnglishBasic => english_basic_tokens(text),
        }
    }

    /// The retrieval terms for `text`: engine-safe (alphanumeric, no embedded
    /// separators) so they round-trip through the engine's full-text tokenizer
    /// unchanged.
    ///
    /// For `default`/`simple`/`ascii_fold` this is exactly the token texts from
    /// [`analyze`](Analyzer::analyze) (each is already a single alphanumeric run).
    /// For `keyword` the whole field collapses to a single normalized term
    /// (alphanumeric, lowercased) so that exact whole-field matching survives the
    /// engine's tokenizer; this is documented in `docs/SEARCH_AND_RANKING.md`.
    pub fn index_terms(self, text: &str) -> Vec<String> {
        match self.preset {
            AnalyzerPreset::Default
            | AnalyzerPreset::Simple
            | AnalyzerPreset::AsciiFold
            | AnalyzerPreset::EnglishBasic => {
                self.analyze(text).into_iter().map(|t| t.text).collect()
            }
            AnalyzerPreset::Keyword => {
                let term: String = text
                    .chars()
                    .filter(|c| c.is_alphanumeric())
                    .flat_map(char::to_lowercase)
                    .collect();
                if term.is_empty() {
                    Vec::new()
                } else {
                    vec![term]
                }
            }
        }
    }

    /// Map a single [`AnalyzerPreset::Default`] token (already lowercased, a single
    /// alphanumeric run) to the term(s) this analyzer would index it as.
    ///
    /// Returns `None` for analyzers that are **not** a per-token transform of the
    /// default tokenizer (currently only [`AnalyzerPreset::Keyword`], which collapses
    /// the whole field). For per-token analyzers it returns the output terms for that
    /// one token: the same token for `default`/`simple`, the ASCII-folded form for
    /// `ascii_fold`, and either nothing (a dropped stopword) or the plural-folded form
    /// for `english_basic`.
    ///
    /// The live search engine uses this to evaluate a non-default analyzer over the
    /// persisted default-tokenized postings, so retrieval matches the offline
    /// `search eval` harness (which analyzes the corpus text up front) without
    /// changing the index or storage format.
    pub fn map_default_token(self, token: &str) -> Option<Vec<String>> {
        match self.preset {
            AnalyzerPreset::Default | AnalyzerPreset::Simple => Some(vec![token.to_string()]),
            AnalyzerPreset::AsciiFold => Some(vec![ascii_fold(token)]),
            AnalyzerPreset::EnglishBasic => {
                if is_english_stopword(token) {
                    Some(Vec::new())
                } else {
                    Some(vec![fold_plural(token)])
                }
            }
            AnalyzerPreset::Keyword => None,
        }
    }
}

/// Split `text` into lowercased alphanumeric-run tokens, optionally ASCII-folding
/// each run. Token offsets are the byte span of the run in the original input. This
/// is the shared engine of the `default`/`simple` (no fold) and `ascii_fold` (fold)
/// presets and matches [`crate::tokenize`] byte-for-byte in token text when folding
/// is off.
fn split_alphanumeric(text: &str, fold: bool) -> TokenStream {
    let mut out = TokenStream::new();
    let mut run_start: Option<usize> = None;
    for (i, c) in text.char_indices() {
        if c.is_alphanumeric() {
            if run_start.is_none() {
                run_start = Some(i);
            }
        } else if let Some(start) = run_start.take() {
            out.push(make_run_token(&text[start..i], start, i, fold));
        }
    }
    if let Some(start) = run_start {
        out.push(make_run_token(&text[start..], start, text.len(), fold));
    }
    out
}

fn make_run_token(run: &str, start: usize, end: usize, fold: bool) -> Token {
    let lowered = run.to_lowercase();
    let text = if fold { ascii_fold(&lowered) } else { lowered };
    Token { text, start, end }
}

/// The `keyword` token: the trimmed input as a single lowercased token, with byte
/// offsets spanning the trimmed source. Whitespace-only (or empty) input yields no
/// token.
fn keyword_token(text: &str) -> Option<Token> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Offsets of the trimmed span within the original input.
    let start = text.len() - text.trim_start().len();
    let end = start + trimmed.len();
    Some(Token {
        text: trimmed.to_lowercase(),
        start,
        end,
    })
}

/// Fold common Latin-1 / Latin Extended-A diacritics to their ASCII base letters.
///
/// This is a fixed, deterministic table — not a language model and not a Unicode
/// normalization library. Characters outside the table pass through unchanged, so
/// non-Latin scripts are preserved rather than mangled. Input is expected to be
/// lowercased already (the analyzer lowercases before folding).
fn ascii_fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => out.push('a'),
            'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => out.push('c'),
            'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => out.push('e'),
            'ì' | 'í' | 'î' | 'ï' | 'ī' | 'ĭ' | 'į' | 'ı' => out.push('i'),
            'ñ' | 'ń' | 'ņ' | 'ň' => out.push('n'),
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => out.push('o'),
            'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => out.push('u'),
            'ý' | 'ÿ' => out.push('y'),
            'ß' => out.push_str("ss"),
            'æ' => out.push_str("ae"),
            'œ' => out.push_str("oe"),
            other => out.push(other),
        }
    }
    out
}

/// A small, fixed English stopword list for [`AnalyzerPreset::EnglishBasic`]. This is
/// deliberately tiny and hand-picked — common function words only — not a
/// linguistic resource. It is fixed and deterministic; it makes no completeness
/// claim.
const ENGLISH_STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "but", "by", "for", "if", "in", "into", "is", "it",
    "no", "not", "of", "on", "or", "such", "that", "the", "their", "then", "there", "these",
    "they", "this", "to", "was", "will", "with",
];

/// Whether `token` (already lowercased) is in the built-in English stopword list.
fn is_english_stopword(token: &str) -> bool {
    ENGLISH_STOPWORDS.binary_search(&token).is_ok()
}

/// Conservatively fold a small set of regular English plural suffixes on an
/// already-lowercased token. This is **not** a stemmer: it only handles a few safe,
/// fully-tested cases and never touches `-ing`/`-ed` or irregular plurals.
///
/// Rules, in order (first match wins):
/// * `…ies` (stem len ≥ 2) → `…y`   (`policies` → `policy`, `queries` → `query`)
/// * `…(s|x|z|ch|sh)es` (stem len ≥ 2) → drop `es`  (`boxes` → `box`, `classes` → `class`)
/// * trailing `s` → drop `s`, but **only** when it is a length-safe regular plural:
///   the token is ≥ 4 bytes, the stem left behind is ≥ 3 bytes, and the token does
///   not end in a protected suffix (`ss`/`us`/`is`/`ns`). The `ns` guard is what
///   keeps bare-`s` singulars like `lens` intact (`lens` stays `lens`, not `len`).
///
/// Anything else is returned unchanged. Tokens that are not pure ASCII letters are
/// returned unchanged, so digits and non-Latin scripts are never mangled.
///
/// The protected endings deliberately cost some recall on a handful of genuine
/// plurals (e.g. `plans` is left as `plans`) in exchange for never collapsing a
/// singular noun onto a wrong stem. The fold is applied symmetrically to the query
/// and the corpus, so whatever it does, both sides reduce to the same term.
fn fold_plural(token: &str) -> String {
    if token.len() < 4 || !token.bytes().all(|b| b.is_ascii_lowercase()) {
        return token.to_string();
    }
    if let Some(stem) = token.strip_suffix("ies") {
        if stem.len() >= 2 {
            return format!("{stem}y");
        }
    }
    if let Some(stem) = token.strip_suffix("es") {
        // Only the `-es` plural that follows a sibilant; otherwise the trailing-`s`
        // rule below (or no change) applies.
        if stem.len() >= 2
            && (stem.ends_with('s')
                || stem.ends_with('x')
                || stem.ends_with('z')
                || stem.ends_with("ch")
                || stem.ends_with("sh"))
        {
            return stem.to_string();
        }
    }
    // Bare trailing `-s`: only fold when the remaining stem is still a real word
    // (≥ 3 bytes) and the word does not end in a protected suffix. The `ns` guard
    // protects singulars like `lens`/`bonus`-style endings from being truncated.
    if token.ends_with('s')
        && !token.ends_with("ss")
        && !token.ends_with("us")
        && !token.ends_with("is")
        && !token.ends_with("ns")
        && token.len() > 3
    {
        return token[..token.len() - 1].to_string();
    }
    token.to_string()
}

/// Tokenize `text` for [`AnalyzerPreset::EnglishBasic`]: simple alphanumeric runs,
/// lowercased, with built-in stopwords dropped and a conservative plural fold
/// applied. Offsets index the original input (so a highlighter underlines the
/// untouched source word, e.g. `backups`).
fn english_basic_tokens(text: &str) -> TokenStream {
    split_alphanumeric(text, false)
        .into_iter()
        .filter(|t| !is_english_stopword(&t.text))
        .map(|t| Token {
            text: fold_plural(&t.text),
            start: t.start,
            end: t.end,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms(a: Analyzer, text: &str) -> Vec<String> {
        a.analyze(text).into_iter().map(|t| t.text).collect()
    }

    #[test]
    fn analyzer_default_matches_existing_behavior() {
        // The `default` preset must emit exactly what the engine's `tokenize` does,
        // term-for-term, for arbitrary inputs — this is the behavior-preservation
        // guarantee the rest of the release depends on.
        let analyzer = Analyzer::new(AnalyzerPreset::Default);
        for input in [
            "Hello, World!",
            "  spaced   OUT text  ",
            "café NAÏVE 123",
            "kebab-case_snake.dotted",
            "",
            "!!!",
            "MixedCASE word2word",
        ] {
            assert_eq!(
                terms(analyzer, input),
                crate::tokenize(input),
                "default analyzer diverged from tokenize() on {input:?}"
            );
            // index_terms() drives retrieval and must also equal tokenize().
            assert_eq!(analyzer.index_terms(input), crate::tokenize(input));
        }
    }

    #[test]
    fn analyzer_simple_lowercases() {
        let analyzer = Analyzer::new(AnalyzerPreset::Simple);
        assert_eq!(terms(analyzer, "Hello WORLD"), vec!["hello", "world"]);
        // simple emits the same terms as default on every input.
        for input in ["A B c", "Refund-Policy 2024", "  x  "] {
            assert_eq!(terms(analyzer, input), crate::tokenize(input));
        }
    }

    #[test]
    fn analyzer_keyword_single_token() {
        let analyzer = Analyzer::new(AnalyzerPreset::Keyword);
        let toks = analyzer.analyze("  Backup Restore  ");
        assert_eq!(toks.len(), 1);
        assert_eq!(toks[0].text, "backup restore");
        // The retrieval term is a single normalized blob (whitespace removed) so it
        // survives the engine tokenizer for exact whole-field matching.
        assert_eq!(
            analyzer.index_terms("Backup Restore"),
            vec!["backuprestore"]
        );
        assert_eq!(
            analyzer.index_terms("backup and restore"),
            vec!["backupandrestore"]
        );
        assert_ne!(
            analyzer.index_terms("Backup Restore"),
            analyzer.index_terms("backup and restore")
        );
    }

    #[test]
    fn analyzer_ascii_fold_if_implemented() {
        let analyzer = Analyzer::new(AnalyzerPreset::AsciiFold);
        assert_eq!(terms(analyzer, "Café NAÏVE"), vec!["cafe", "naive"]);
        assert_eq!(terms(analyzer, "Mötley Crüe"), vec!["motley", "crue"]);
        assert_eq!(terms(analyzer, "Straße"), vec!["strasse"]);
        // Non-Latin scripts pass through unchanged (no mangling, no claim of support).
        assert_eq!(terms(analyzer, "naïve café"), vec!["naive", "cafe"]);
    }

    #[test]
    fn analyzer_offsets_are_stable() {
        // Offsets always index the ORIGINAL bytes, even when the emitted text was
        // folded or lowercased to a different length.
        let analyzer = Analyzer::new(AnalyzerPreset::AsciiFold);
        let input = "Café shop";
        let toks = analyzer.analyze(input);
        assert_eq!(toks.len(), 2);
        // "Café" occupies bytes 0..5 ('é' is two bytes); folds to "cafe".
        assert_eq!(toks[0].text, "cafe");
        assert_eq!(&input[toks[0].start..toks[0].end], "Café");
        assert_eq!(toks[1].text, "shop");
        assert_eq!(&input[toks[1].start..toks[1].end], "shop");
        // Determinism: analyzing twice yields identical streams.
        assert_eq!(analyzer.analyze(input), toks);
    }

    #[test]
    fn analyzer_empty_input() {
        for preset in AnalyzerPreset::ALL {
            let analyzer = Analyzer::new(preset);
            assert!(
                analyzer.analyze("").is_empty(),
                "{} on empty",
                preset.name()
            );
            assert!(
                analyzer.analyze("   \t\n ").is_empty(),
                "{} on whitespace",
                preset.name()
            );
            assert!(analyzer.index_terms("").is_empty());
        }
    }

    #[test]
    fn analyzer_punctuation() {
        let analyzer = Analyzer::new(AnalyzerPreset::Simple);
        assert_eq!(
            terms(analyzer, "well-formed, JSON: {\"a\": 1}"),
            vec!["well", "formed", "json", "a", "1"]
        );
        // Pure punctuation yields no tokens.
        assert!(analyzer.analyze("!@#$%^&*()").is_empty());
    }

    #[test]
    fn analyzer_unicode_basic() {
        // Unicode alphanumerics are tokens; the default/simple presets lowercase
        // but do not fold, so accents survive there.
        let simple = Analyzer::new(AnalyzerPreset::Simple);
        assert_eq!(terms(simple, "Café"), vec!["café"]);
        // CJK characters are alphanumeric and form tokens (split on the space).
        assert_eq!(terms(simple, "東京 tokyo"), vec!["東京", "tokyo"]);
    }

    #[test]
    fn analyzer_does_not_panic() {
        // Adversarial inputs: lone combining marks, mixed scripts, long runs, and
        // multibyte boundaries must never panic or produce invalid offsets.
        let inputs = [
            "\u{0301}\u{0301}",
            "a\u{200B}b",
            &"x".repeat(10_000),
            "🚀 rocket 🌟 star",
            "ﬁ ligature",
            "\0null\0",
        ];
        for preset in AnalyzerPreset::ALL {
            let analyzer = Analyzer::new(preset);
            for input in inputs {
                let toks = analyzer.analyze(input);
                for t in &toks {
                    // Offsets must be valid char boundaries into the original input.
                    assert!(input.is_char_boundary(t.start));
                    assert!(input.is_char_boundary(t.end));
                    assert!(t.start <= t.end);
                }
            }
        }
    }

    #[test]
    fn unknown_analyzer_is_structured_error() {
        let err = AnalyzerPreset::parse("stemming").unwrap_err();
        assert!(matches!(err, Error::InvalidRequest(_)));
        let msg = err.to_string();
        assert!(msg.contains("unknown analyzer"));
        assert!(msg.contains("default"));
        assert!(msg.contains("english_basic"));
    }

    fn english() -> Analyzer {
        Analyzer::new(AnalyzerPreset::EnglishBasic)
    }

    #[test]
    fn english_basic_removes_common_stopwords() {
        // The built-in stopwords are dropped; the content words survive.
        assert_eq!(
            terms(english(), "the quick brown fox and a lazy dog"),
            vec!["quick", "brown", "fox", "lazy", "dog"]
        );
        // A query of only stopwords yields no terms.
        assert!(english().analyze("the and of to").is_empty());
        assert!(english().index_terms("the and of to").is_empty());
    }

    #[test]
    fn english_basic_preserves_meaningful_tokens() {
        // Non-stopword tokens are never dropped, and tokens that are not regular
        // plurals are returned unchanged (no over-stemming).
        assert_eq!(
            terms(english(), "restore backup status analysis bus"),
            vec!["restore", "backup", "status", "analysis", "bus"]
        );
        // Determinism.
        assert_eq!(
            english().analyze("Restore the Backup"),
            english().analyze("Restore the Backup")
        );
    }

    #[test]
    fn english_basic_suffix_normalization_if_implemented() {
        // Conservative, fully-specified plural fold.
        assert_eq!(terms(english(), "backups"), vec!["backup"]);
        assert_eq!(terms(english(), "documents"), vec!["document"]);
        assert_eq!(terms(english(), "boxes"), vec!["box"]);
        assert_eq!(terms(english(), "classes"), vec!["class"]);
        assert_eq!(terms(english(), "policies"), vec!["policy"]);
        assert_eq!(terms(english(), "watches"), vec!["watch"]);
        // The `-ss`/`-us`/`-is` guards protect the common false positives so these
        // singular words are never stripped.
        for word in ["status", "analysis", "class", "bus", "process", "basis"] {
            assert_eq!(
                terms(english(), word),
                vec![word.to_string()],
                "{word} must not be over-folded"
            );
        }
        // The fold is conservative: a bare `-s` singular like "lens" is left intact
        // by the `ns` protected-ending guard, so it is never truncated to "len".
        assert_eq!(terms(english(), "lens"), vec!["lens"]);
        // A singular query and its plural form normalize to the same term, which is
        // what makes recall work over the default-tokenized index.
        assert_eq!(
            english().index_terms("backup"),
            english().index_terms("backups")
        );
    }

    #[test]
    fn english_basic_keeps_lens() {
        // Regression: the bare-`s` singular "lens" must NOT fold to "len".
        assert_eq!(terms(english(), "lens"), vec!["lens"]);
        assert_eq!(english().index_terms("lens"), vec!["lens"]);
        // Other `-ns` singulars are likewise protected.
        assert_eq!(terms(english(), "bonus"), vec!["bonus"]);
    }

    #[test]
    fn english_basic_keeps_status() {
        // `-us` guard.
        assert_eq!(terms(english(), "status"), vec!["status"]);
        assert_eq!(english().index_terms("status"), vec!["status"]);
    }

    #[test]
    fn english_basic_keeps_analysis() {
        // `-is` guard.
        assert_eq!(terms(english(), "analysis"), vec!["analysis"]);
        assert_eq!(english().index_terms("analysis"), vec!["analysis"]);
    }

    #[test]
    fn english_basic_keeps_class() {
        // `-ss` guard.
        assert_eq!(terms(english(), "class"), vec!["class"]);
        assert_eq!(english().index_terms("class"), vec!["class"]);
    }

    #[test]
    fn english_basic_folds_backups() {
        // Clear regular plural still folds (`-ps` is not a protected ending).
        assert_eq!(terms(english(), "backups"), vec!["backup"]);
        assert_eq!(english().index_terms("backups"), vec!["backup"]);
        // And "backup" itself is left unchanged (no trailing `s`).
        assert_eq!(terms(english(), "backup"), vec!["backup"]);
        // restores -> restore via the length-safe bare-`s` rule.
        assert_eq!(terms(english(), "restores"), vec!["restore"]);
    }

    #[test]
    fn english_basic_folds_queries_if_supported() {
        // `-ies` -> `-y` for length-safe terms.
        assert_eq!(terms(english(), "queries"), vec!["query"]);
        assert_eq!(english().index_terms("queries"), vec!["query"]);
        // Singular "query" is untouched, so query/queries share a term.
        assert_eq!(
            english().index_terms("query"),
            english().index_terms("queries")
        );
    }

    #[test]
    fn english_basic_stopwords_still_removed() {
        // The stopword list keeps working after the plural-fold change.
        assert_eq!(
            terms(english(), "the status of a lens and the backups"),
            vec!["status", "lens", "backup"]
        );
        assert!(english().analyze("the and of to is it").is_empty());
    }

    #[test]
    fn english_basic_not_full_stemmer() {
        // Still intentionally not a stemmer or NLP model.
        let a = english();
        assert_eq!(terms(a, "running"), vec!["running"]);
        assert_eq!(terms(a, "studied"), vec!["studied"]);
        assert_eq!(terms(a, "better"), vec!["better"]);
        assert_eq!(terms(a, "mice"), vec!["mice"]);
        // The protected-ending guards are mechanical, not lexical: a genuine `-ns`
        // plural like "plans" is left intact too, which is the conservative tradeoff.
        assert_eq!(terms(a, "plans"), vec!["plans"]);
    }

    #[test]
    fn english_basic_empty_input() {
        assert!(english().analyze("").is_empty());
        assert!(english().analyze("   \t\n ").is_empty());
        assert!(english().index_terms("").is_empty());
        assert!(english().analyze("!!! ??? ...").is_empty());
    }

    #[test]
    fn english_basic_offsets_stable_or_documented() {
        // Offsets index the ORIGINAL source word even though the emitted term was
        // folded (e.g. "Backups" -> "backup"), so a highlighter underlines "Backups".
        let input = "Verify the Backups now";
        let toks = english().analyze(input);
        assert_eq!(
            toks.iter().map(|t| t.text.as_str()).collect::<Vec<_>>(),
            vec!["verify", "backup", "now"]
        );
        // "Backups" is the second emitted token; its span covers the original word.
        let backup = &toks[1];
        assert_eq!(&input[backup.start..backup.end], "Backups");
    }

    #[test]
    fn english_basic_no_full_nlp_claim() {
        // english_basic is intentionally NOT a stemmer or language model. These
        // behaviors prove it: `-ing`/`-ed` verb forms, comparatives, and irregular
        // plurals are all left untouched (full NLP would normalize them).
        let a = english();
        assert_eq!(terms(a, "running"), vec!["running"]); // no -ing handling
        assert_eq!(terms(a, "studied"), vec!["studied"]); // no -ed handling
        assert_eq!(terms(a, "better"), vec!["better"]); // no lemmatization
        assert_eq!(terms(a, "mice"), vec!["mice"]); // no irregular-plural model
    }

    #[test]
    fn map_default_token_matches_index_terms() {
        // For every per-token analyzer, mapping the default tokens of an input and
        // flattening must equal index_terms() on the same input. This is the
        // invariant the live engine relies on to search over default postings.
        let inputs = [
            "The Café serves Backups and policies",
            "Mötley Crüe boxes",
            "restore the backup then verify",
            "",
            "STATUS analysis",
        ];
        for preset in [
            AnalyzerPreset::Default,
            AnalyzerPreset::Simple,
            AnalyzerPreset::AsciiFold,
            AnalyzerPreset::EnglishBasic,
        ] {
            let a = Analyzer::new(preset);
            assert!(preset.is_per_token());
            for input in inputs {
                let via_map: Vec<String> = crate::tokenize(input)
                    .iter()
                    .flat_map(|tok| a.map_default_token(tok).expect("per-token analyzer"))
                    .collect();
                assert_eq!(
                    via_map,
                    a.index_terms(input),
                    "{} on {input:?}",
                    preset.name()
                );
            }
        }
        // Keyword is whole-field: not a per-token transform.
        assert!(!AnalyzerPreset::Keyword.is_per_token());
        assert!(Analyzer::new(AnalyzerPreset::Keyword)
            .map_default_token("anything")
            .is_none());
    }
}
