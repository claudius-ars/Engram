use std::path::{Path, PathBuf};

use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{Field, IndexRecordOption, Schema, Value};
use tantivy::{DocAddress, Index, IndexReader, Searcher, TantivyDocument};

use engram_core::temporal::NULL_TIMESTAMP;
use engram_core::WorkspaceConfig;

use crate::causal_reader::CausalReader;
use crate::result::QueryHit;

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

/// Resolved schema fields used by search and enrichment.
struct ResolvedFields {
    f_title: Field,
    f_body: Field,
    f_tags: Field,
    f_keywords: Field,
    f_domain_tags: Field,
    f_id: Field,
    f_source_path: Field,
    f_caused_by: Field,
    f_causes: Field,
    f_related: Field,
    f_maturity: Field,
    f_access_count: Field,
    f_update_count: Field,
}

impl ResolvedFields {
    fn resolve(schema: &Schema) -> Result<Self, SearchError> {
        Ok(ResolvedFields {
            f_title: schema.get_field("title").map_err(|e| SearchError::Schema(e.to_string()))?,
            f_body: schema.get_field("body").map_err(|e| SearchError::Schema(e.to_string()))?,
            f_tags: schema.get_field("tags").map_err(|e| SearchError::Schema(e.to_string()))?,
            f_keywords: schema.get_field("keywords").map_err(|e| SearchError::Schema(e.to_string()))?,
            f_domain_tags: schema.get_field("domain_tags").map_err(|e| SearchError::Schema(e.to_string()))?,
            f_id: schema.get_field("id").map_err(|e| SearchError::Schema(e.to_string()))?,
            f_source_path: schema.get_field("source_path").map_err(|e| SearchError::Schema(e.to_string()))?,
            f_caused_by: schema.get_field("caused_by").map_err(|e| SearchError::Schema(e.to_string()))?,
            f_causes: schema.get_field("causes").map_err(|e| SearchError::Schema(e.to_string()))?,
            f_related: schema.get_field("related").map_err(|e| SearchError::Schema(e.to_string()))?,
            f_maturity: schema.get_field("maturity").map_err(|e| SearchError::Schema(e.to_string()))?,
            f_access_count: schema.get_field("access_count").map_err(|e| SearchError::Schema(e.to_string()))?,
            f_update_count: schema.get_field("update_count").map_err(|e| SearchError::Schema(e.to_string()))?,
        })
    }
}

/// An opened Tantivy index with resolved schema fields.
///
/// Created once per query session via `BM25Searcher::open()`. Both BM25
/// search and temporal hit enrichment operate on this handle, avoiding
/// a second `open_in_dir` call.
pub struct OpenIndex {
    index: Index,
    reader: IndexReader,
    fields: ResolvedFields,
}

impl OpenIndex {
    /// Get the Tantivy `Searcher` for this index snapshot.
    fn searcher(&self) -> Searcher {
        self.reader.searcher()
    }

    /// Enrich a sparse temporal hit by looking up the matching document
    /// in the Tantivy index via `source_path_hash`.
    ///
    /// On any failure (malformed source_path, Tantivy error, no matching
    /// document), returns the original hit unchanged. Never returns Err.
    pub fn enrich_temporal_hit(&self, hit: QueryHit) -> QueryHit {
        // Parse the hex hash from "<temporal:XXXXXXXXXXXXXXXX>"
        let hash_hex = match hit.source_path.strip_prefix("<temporal:")
            .and_then(|s| s.strip_suffix('>'))
        {
            Some(h) if h.len() == 16 => h,
            _ => return hit, // not a temporal hit or malformed → return unchanged
        };

        // PHASE 3 LIMITATION: O(N) segment scan.
        //
        // Enrichment matches by FNV-1a hash of source_path, but Tantivy's source_path
        // field stores the string, not the hash. TermQuery requires the exact string —
        // the hash cannot be inverted. A full segment scan is therefore required.
        //
        // PHASE 4 FIX: Add source_path_hash as a u64 FAST field in the Tantivy schema
        // (schema version bump 2 → 3). The enrichment can then use the column reader
        // API for an O(1) lookup:
        //   let col = fast_fields.u64("source_path_hash")?;
        //   let doc_id = col.iter().position(|h| h == target_hash)?;
        // This requires a wipe-and-rebuild migration for existing indexes.

        let target_hash = match u64::from_str_radix(hash_hex, 16) {
            Ok(h) => h,
            Err(_) => return hit,
        };

        let searcher = self.searcher();

        // O(N) scan — acceptable for corpus ≤5,000 facts (Phase 3 target).
        // See PHASE 3 LIMITATION comment above for the Phase 4 fix path.
        let doc_count = searcher.num_docs();
        debug_assert!(
            doc_count <= 5_000,
            "enrich_temporal_hit O(N) scan: {} docs exceeds Phase 3 target of 5,000",
            doc_count
        );

        for segment_ord in 0..searcher.segment_readers().len() {
            let segment = searcher.segment_reader(segment_ord as u32);
            let alive_bitset = segment.alive_bitset();

            for doc_id in 0..segment.max_doc() {
                // Skip deleted docs
                if let Some(bitset) = &alive_bitset {
                    if !bitset.is_alive(doc_id) {
                        continue;
                    }
                }

                let addr = DocAddress::new(segment_ord as u32, doc_id);
                let doc: TantivyDocument = match searcher.doc(addr) {
                    Ok(d) => d,
                    Err(_) => continue,
                };

                let source_path = doc
                    .get_first(self.fields.f_source_path)
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                if engram_core::temporal::fnv1a_64(source_path.as_bytes()) != target_hash {
                    continue;
                }

                // Found it — enrich the hit with Tantivy data
                let id = doc
                    .get_first(self.fields.f_id)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let title = doc
                    .get_first(self.fields.f_title)
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .filter(|s| !s.is_empty());
                let tags: Vec<String> = doc
                    .get_first(self.fields.f_tags)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .split_whitespace()
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect();
                let domain_tags: Vec<String> = doc
                    .get_first(self.fields.f_domain_tags)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .split_whitespace()
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect();
                let keywords: Vec<String> = doc
                    .get_first(self.fields.f_keywords)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .split_whitespace()
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect();
                let caused_by: Vec<String> = doc
                    .get_first(self.fields.f_caused_by)
                    .and_then(|v| v.as_str())
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or_default();
                let causes: Vec<String> = doc
                    .get_first(self.fields.f_causes)
                    .and_then(|v| v.as_str())
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or_default();
                let related: Vec<String> = doc
                    .get_first(self.fields.f_related)
                    .and_then(|v| v.as_str())
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or_default();
                let maturity = doc
                    .get_first(self.fields.f_maturity)
                    .and_then(|v| v.as_f64())
                    .unwrap_or(1.0);
                let access_count = doc
                    .get_first(self.fields.f_access_count)
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let update_count = doc
                    .get_first(self.fields.f_update_count)
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);

                // FAST fields via column reader
                let importance = read_fast_f64(&searcher, addr, "importance");
                let confidence = read_fast_f64(&searcher, addr, "confidence");
                let recency = read_fast_f64(&searcher, addr, "recency");
                let fact_type_int = read_fast_u64(&searcher, addr, "fact_type_int");

                let fact_type = match fact_type_int {
                    0 => "durable",
                    1 => "state",
                    2 => "event",
                    _ => "durable",
                }
                .to_string();

                // PHASE 4 TODO: access_count should be incremented here to record
                // that this fact was accessed via the query pipeline. The query read
                // path (engram-query) cannot write to Tantivy. Phase 4 (Bulwark
                // integration) is the correct place to implement the write-back via
                // an append-only access log.

                return QueryHit {
                    id,
                    title,
                    source_path: source_path.to_string(),
                    tags,
                    domain_tags,
                    score: hit.score, // preserve temporal score
                    bm25_score: hit.bm25_score, // preserve original (0.0)
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
                };
            }
        }

        // No matching document found (stale/deleted fact) — return sparse hit unchanged
        hit
    }
}

impl BM25Searcher {
    pub fn new(index_dir: &Path) -> Self {
        BM25Searcher {
            index_dir: index_dir.to_path_buf(),
        }
    }

    /// Open the Tantivy index and resolve schema fields. Called once per
    /// query session. Returns `None` if the index does not exist.
    pub fn open(&self) -> Result<OpenIndex, SearchError> {
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
        let fields = ResolvedFields::resolve(&schema)?;
        let reader = index.reader()?;

        Ok(OpenIndex {
            index,
            reader,
            fields,
        })
    }

    pub fn search(
        &self,
        query_string: &str,
        options: &crate::QueryOptions,
        config: &WorkspaceConfig,
        causal_reader: &CausalReader,
        anchor_fact_id: Option<&str>,
    ) -> Result<Vec<ScoredDoc>, SearchError> {
        let open = self.open()?;
        open.search_with(query_string, options, config, causal_reader, anchor_fact_id)
    }
}

impl OpenIndex {
    pub fn search_with(
        &self,
        query_string: &str,
        options: &crate::QueryOptions,
        config: &WorkspaceConfig,
        causal_reader: &CausalReader,
        anchor_fact_id: Option<&str>,
    ) -> Result<Vec<ScoredDoc>, SearchError> {
        let f = &self.fields;

        // Build query parser with field boosts
        let mut query_parser = QueryParser::for_index(
            &self.index,
            vec![f.f_title, f.f_body, f.f_tags, f.f_keywords, f.f_domain_tags, f.f_id],
        );
        query_parser.set_field_boost(f.f_title, 3.0);
        query_parser.set_field_boost(f.f_body, 1.0);
        query_parser.set_field_boost(f.f_tags, 2.0);
        query_parser.set_field_boost(f.f_keywords, 2.0);
        query_parser.set_field_boost(f.f_domain_tags, 1.5);
        query_parser.set_field_boost(f.f_id, 1.0);

        // Parse query; fall back to term query on title on failure
        let query = match query_parser.parse_query(query_string) {
            Ok(q) => q,
            Err(_) => {
                let term = tantivy::Term::from_field_text(f.f_title, query_string);
                Box::new(tantivy::query::TermQuery::new(
                    term,
                    IndexRecordOption::WithFreqsAndPositions,
                ))
            }
        };

        let searcher = self.searcher();

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
                    let causal_adj = match anchor_fact_id {
                        Some(anchor) => {
                            let candidate_id = doc
                                .get_first(f.f_id)
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            causal_reader.causal_adjacency(
                                anchor,
                                candidate_id,
                                config.causal_max_hops,
                            )
                        }
                        None => 1.0, // no anchor yet (first pass) → neutral
                    };
                    bm25_normalized
                        * confidence
                        * importance
                        * recency
                        * causal_adj
                }

                // Unknown — treat as durable
                _ => bm25_normalized * confidence * importance,
            };

            // Reconstruct stored fields
            let id = doc
                .get_first(f.f_id)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let title = doc
                .get_first(f.f_title)
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty());
            let source_path = doc
                .get_first(f.f_source_path)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let tags: Vec<String> = doc
                .get_first(f.f_tags)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .split_whitespace()
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect();
            let domain_tags: Vec<String> = doc
                .get_first(f.f_domain_tags)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .split_whitespace()
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect();
            let keywords: Vec<String> = doc
                .get_first(f.f_keywords)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .split_whitespace()
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect();
            let caused_by: Vec<String> = doc
                .get_first(f.f_caused_by)
                .and_then(|v| v.as_str())
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default();
            let causes: Vec<String> = doc
                .get_first(f.f_causes)
                .and_then(|v| v.as_str())
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default();
            let related: Vec<String> = doc
                .get_first(f.f_related)
                .and_then(|v| v.as_str())
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default();
            let maturity = doc
                .get_first(f.f_maturity)
                .and_then(|v| v.as_f64())
                .unwrap_or(1.0);
            let access_count = doc
                .get_first(f.f_access_count)
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let update_count = doc
                .get_first(f.f_update_count)
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
