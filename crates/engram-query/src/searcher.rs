use std::path::{Path, PathBuf};

use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::Value;
use tantivy::{DocAddress, Index, Searcher, TantivyDocument};

use engram_core::temporal::NULL_TIMESTAMP;
use engram_core::WorkspaceConfig;

use crate::result::QueryHit;

/// Causal adjacency multiplier for event facts. Phase 2 default = 1.0.
/// Phase 3 replaces this with actual causal graph adjacency values.
pub const CAUSAL_ADJACENCY_DEFAULT: f64 = 1.0;

/// Exponential decay half-life for the freshness bonus (days).
pub const FRESHNESS_HALF_LIFE_DAYS: f64 = 30.0;

/// Amplitude of the freshness bonus. Just-updated = 1.0 + AMPLITUDE.
pub const FRESHNESS_AMPLITUDE: f64 = 0.5;

#[derive(Debug)]
pub struct ScoredDoc {
    pub tantivy_score: f32,
    pub compound_score: f64,
    pub hit: QueryHit,
}

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error("index not found at {0}")]
    IndexNotFound(PathBuf),

    #[error("tantivy error: {0}")]
    Tantivy(#[from] tantivy::TantivyError),

    #[error("schema error: {0}")]
    Schema(String),
}

pub struct BM25Searcher {
    index_dir: PathBuf,
}

/// Read a FAST f64 field value from a document address.
fn read_fast_f64(searcher: &Searcher, addr: DocAddress, field: &str) -> f64 {
    let segment = searcher.segment_reader(addr.segment_ord);
    let reader = segment.fast_fields();
    reader
        .f64(field)
        .ok()
        .and_then(|col| col.first(addr.doc_id))
        .unwrap_or(1.0)
}

/// Read a FAST u64 field value from a document address.
fn read_fast_u64(searcher: &Searcher, addr: DocAddress, field: &str) -> u64 {
    let segment = searcher.segment_reader(addr.segment_ord);
    let reader = segment.fast_fields();
    reader
        .u64(field)
        .ok()
        .and_then(|col| col.first(addr.doc_id))
        .unwrap_or(0)
}

/// Read a FAST i64 field value from a document address.
fn read_fast_i64(searcher: &Searcher, addr: DocAddress, field: &str) -> i64 {
    let segment = searcher.segment_reader(addr.segment_ord);
    let reader = segment.fast_fields();
    reader
        .i64(field)
        .ok()
        .and_then(|col| col.first(addr.doc_id))
        .unwrap_or(NULL_TIMESTAMP)
}

/// Exponential decay freshness bonus for state facts.
///
/// Returns a multiplier in [1.0, 1.0 + FRESHNESS_AMPLITUDE]:
/// - Just updated (0 days): 1.0 + 0.5 = 1.5
/// - 30 days ago: ≈ 1.184
/// - 90 days ago: ≈ 1.025
/// - 365+ days ago: effectively 1.0
pub fn freshness_bonus(updated_at_ts: i64, now_ts: i64) -> f64 {
    if updated_at_ts == NULL_TIMESTAMP || updated_at_ts <= 0 {
        return 1.0; // no timestamp — no bonus, no penalty
    }
    let days_since = (now_ts - updated_at_ts).max(0) as f64 / 86_400.0;
    1.0 + FRESHNESS_AMPLITUDE * (-days_since / FRESHNESS_HALF_LIFE_DAYS).exp()
}

impl BM25Searcher {
    pub fn new(index_dir: &Path) -> Self {
        BM25Searcher {
            index_dir: index_dir.to_path_buf(),
        }
    }

    pub fn search(
        &self,
        query_string: &str,
        options: &crate::QueryOptions,
        config: &WorkspaceConfig,
    ) -> Result<Vec<ScoredDoc>, SearchError> {
        if !self.index_dir.exists() {
            return Err(SearchError::IndexNotFound(self.index_dir.clone()));
        }

        let index = Index::open_in_dir(&self.index_dir).map_err(|e| {
            if self.index_dir.join("meta.json").exists() {
                SearchError::Tantivy(e)
            } else {
                SearchError::IndexNotFound(self.index_dir.clone())
            }
        })?;

        let schema = index.schema();

        // Resolve fields
        let f_title = schema.get_field("title").map_err(|e| SearchError::Schema(e.to_string()))?;
        let f_body = schema.get_field("body").map_err(|e| SearchError::Schema(e.to_string()))?;
        let f_tags = schema.get_field("tags").map_err(|e| SearchError::Schema(e.to_string()))?;
        let f_keywords = schema.get_field("keywords").map_err(|e| SearchError::Schema(e.to_string()))?;
        let f_domain_tags = schema.get_field("domain_tags").map_err(|e| SearchError::Schema(e.to_string()))?;
        let f_id = schema.get_field("id").map_err(|e| SearchError::Schema(e.to_string()))?;
        let f_source_path = schema.get_field("source_path").map_err(|e| SearchError::Schema(e.to_string()))?;
        let f_caused_by = schema.get_field("caused_by").map_err(|e| SearchError::Schema(e.to_string()))?;
        let f_causes = schema.get_field("causes").map_err(|e| SearchError::Schema(e.to_string()))?;
        let f_related = schema.get_field("related").map_err(|e| SearchError::Schema(e.to_string()))?;
        let f_maturity = schema.get_field("maturity").map_err(|e| SearchError::Schema(e.to_string()))?;
        let f_access_count = schema.get_field("access_count").map_err(|e| SearchError::Schema(e.to_string()))?;
        let f_update_count = schema.get_field("update_count").map_err(|e| SearchError::Schema(e.to_string()))?;

        // Build query parser with field boosts
        let mut query_parser = QueryParser::for_index(
            &index,
            vec![f_title, f_body, f_tags, f_keywords, f_domain_tags, f_id],
        );
        query_parser.set_field_boost(f_title, 3.0);
        query_parser.set_field_boost(f_body, 1.0);
        query_parser.set_field_boost(f_tags, 2.0);
        query_parser.set_field_boost(f_keywords, 2.0);
        query_parser.set_field_boost(f_domain_tags, 1.5);
        query_parser.set_field_boost(f_id, 1.0);

        // Parse query; fall back to term query on title on failure
        let query = match query_parser.parse_query(query_string) {
            Ok(q) => q,
            Err(_) => {
                let term = tantivy::Term::from_field_text(f_title, query_string);
                Box::new(tantivy::query::TermQuery::new(
                    term,
                    tantivy::schema::IndexRecordOption::WithFreqsAndPositions,
                ))
            }
        };

        let reader = index.reader()?;
        let searcher = reader.searcher();

        let fetch_limit = options.max_results * 2;
        let top_docs = searcher.search(&query, &TopDocs::with_limit(fetch_limit))?;

        if top_docs.is_empty() {
            return Ok(vec![]);
        }

        // Find max BM25 score for normalization
        let max_bm25 = top_docs
            .iter()
            .map(|(score, _)| *score)
            .fold(0.0f32, f32::max);

        // Compute now_ts once for consistent scoring across all documents
        let now_ts = chrono::Utc::now().timestamp();

        let mut scored_docs = Vec::with_capacity(top_docs.len());

        for (bm25_raw, doc_addr) in &top_docs {
            let doc: TantivyDocument = searcher.doc(*doc_addr)?;

            // Normalize BM25 score to [0,1]
            let bm25_normalized = if max_bm25 > 0.0 {
                *bm25_raw as f64 / max_bm25 as f64
            } else {
                0.0
            };

            // Read FAST field values via column readers
            let importance = read_fast_f64(&searcher, *doc_addr, "importance");
            let recency = read_fast_f64(&searcher, *doc_addr, "recency");
            let confidence = read_fast_f64(&searcher, *doc_addr, "confidence");
            let fact_type_int = read_fast_u64(&searcher, *doc_addr, "fact_type_int");
            let valid_until_ts = read_fast_i64(&searcher, *doc_addr, "valid_until_ts");
            let updated_at_ts = read_fast_i64(&searcher, *doc_addr, "updated_at_ts");

            // Fact-type-aware compound scoring
            let compound_score = match fact_type_int {
                // Durable: recency suppressed — architectural decisions do not age
                0 => bm25_normalized * confidence * importance,

                // State: expired facts score 0 (Layer 2: see temporal_reader.rs ENFORCEMENT CONTRACT)
                1 => {
                    if valid_until_ts != NULL_TIMESTAMP && valid_until_ts < now_ts {
                        0.0
                    } else {
                        bm25_normalized
                            * confidence
                            * importance
                            * freshness_bonus(updated_at_ts, now_ts)
                    }
                }

                // Event: includes recency and causal adjacency
                2 => {
                    bm25_normalized
                        * confidence
                        * importance
                        * recency
                        * CAUSAL_ADJACENCY_DEFAULT
                }

                // Unknown — treat as durable
                _ => bm25_normalized * confidence * importance,
            };

            // Reconstruct stored fields
            let id = doc
                .get_first(f_id)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let title = doc
                .get_first(f_title)
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty());
            let source_path = doc
                .get_first(f_source_path)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let tags: Vec<String> = doc
                .get_first(f_tags)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .split_whitespace()
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect();
            let domain_tags: Vec<String> = doc
                .get_first(f_domain_tags)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .split_whitespace()
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect();
            let keywords: Vec<String> = doc
                .get_first(f_keywords)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .split_whitespace()
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect();
            let caused_by: Vec<String> = doc
                .get_first(f_caused_by)
                .and_then(|v| v.as_str())
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default();
            let causes: Vec<String> = doc
                .get_first(f_causes)
                .and_then(|v| v.as_str())
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default();
            let related: Vec<String> = doc
                .get_first(f_related)
                .and_then(|v| v.as_str())
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default();
            let maturity = doc
                .get_first(f_maturity)
                .and_then(|v| v.as_f64())
                .unwrap_or(1.0);
            let access_count = doc
                .get_first(f_access_count)
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let update_count = doc
                .get_first(f_update_count)
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            let fact_type = match fact_type_int {
                0 => "durable",
                1 => "state",
                2 => "event",
                _ => "durable",
            }
            .to_string();

            scored_docs.push(ScoredDoc {
                tantivy_score: *bm25_raw,
                compound_score,
                hit: QueryHit {
                    id,
                    title,
                    source_path,
                    tags,
                    domain_tags,
                    score: compound_score,
                    bm25_score: bm25_normalized,
                    fact_type,
                    confidence,
                    importance,
                    recency,
                    caused_by,
                    causes,
                    keywords,
                    related,
                    maturity,
                    access_count,
                    update_count,
                },
            });
        }

        // Sort by compound score descending
        scored_docs.sort_by(|a, b| {
            b.compound_score
                .partial_cmp(&a.compound_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Filter: discard results below score_threshold
        let scored_docs: Vec<_> = scored_docs
            .into_iter()
            .filter(|d| d.compound_score >= config.score_threshold)
            .collect();

        // Gap enforcement: if top two results are within score_gap of each other,
        // the results are ambiguous — return only the top result.
        let scored_docs = if scored_docs.len() >= 2 {
            let top_score = scored_docs[0].compound_score;
            let second_score = scored_docs[1].compound_score;
            if top_score - second_score < config.score_gap {
                scored_docs.into_iter().take(1).collect()
            } else {
                scored_docs
            }
        } else {
            scored_docs
        };

        // Truncate to max_results
        let scored_docs: Vec<_> = scored_docs
            .into_iter()
            .take(options.max_results)
            .collect();

        Ok(scored_docs)
    }
}
