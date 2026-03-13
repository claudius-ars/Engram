use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{Field, IndexRecordOption, Schema, Value};
use tantivy::{DocAddress, Index, IndexReader, Searcher, TantivyDocument};

use engram_core::temporal::NULL_TIMESTAMP;
use engram_core::{OntologyIndex, WorkspaceConfig};

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
    /// FAST field handle for source_path_hash (FNV-1a u64).
    /// `None` for pre-v3 schemas that lack this column.
    f_source_path_hash: Option<Field>,
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
            f_source_path_hash: schema.get_field("source_path_hash").ok(),
        })
    }
}

/// Maps source_path_hash (FNV-1a u64) → DocAddress for O(1) enrichment
/// lookups. Built per-Searcher snapshot (DocAddress values encode segment
/// ordinals that are only valid for the Searcher they were built from).
pub type DocAddressMap = HashMap<u64, DocAddress>;

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
    pub fn searcher(&self) -> Searcher {
        self.reader.searcher()
    }

    /// The FAST field handle for `source_path_hash`, if the schema is v3+.
    pub fn f_source_path_hash(&self) -> Option<Field> {
        self.fields.f_source_path_hash
    }

    /// Enrich a sparse hit by looking up the matching document in the
    /// Tantivy index.
    ///
    /// Sentinel-prefix dispatch:
    /// - `<causal:...>` → return hit unchanged (synthetic causal hit)
    /// - `<temporal:HASH>` → extract hash, look up document
    /// - `<llm:...>` → return hit unchanged (synthetic LLM hit)
    /// - anything else → compute hash from source_path, look up document
    ///
    /// On any failure (malformed source_path, Tantivy error, no matching
    /// document), returns the original hit unchanged. Never returns Err.
    pub fn enrich_hit(
        &self,
        hit: QueryHit,
        hash_to_doc: Option<&DocAddressMap>,
    ) -> QueryHit {
        // Sentinel dispatch — synthetic hits are returned unchanged
        if hit.source_path.starts_with("<causal:") || hit.source_path.starts_with("<llm:") {
            return hit;
        }

        // Determine the target hash
        let target_hash = if let Some(inner) = hit
            .source_path
            .strip_prefix("<temporal:")
            .and_then(|s| s.strip_suffix('>'))
        {
            // Temporal hit: hash is hex-encoded in the sentinel
            if inner.len() != 16 {
                return hit; // malformed
            }
            match u64::from_str_radix(inner, 16) {
                Ok(h) => h,
                Err(_) => return hit,
            }
        } else {
            // Regular hit: compute hash from source_path
            engram_core::hash::fnv1a_u64(hit.source_path.as_bytes())
        };

        let searcher = self.searcher();

        // O(1) path: use the pre-built DocAddressMap
        if let Some(map) = hash_to_doc {
            if let Some(&addr) = map.get(&target_hash) {
                match searcher.doc(addr) {
                    Ok(doc) => {
                        return apply_doc_enrichment(
                            hit, &doc, &self.fields, &searcher, addr,
                        );
                    }
                    Err(e) => {
                        eprintln!(
                            "WARN: enrich_hit: searcher.doc() failed for {:016x}: {}",
                            target_hash, e
                        );
                        return hit; // NRD-6: never return Err
                    }
                }
            } else {
                // Hash not in map — fact was deleted (NRD-17: keep sparse hit)
                return hit;
            }
        }

        // Fallback O(N) scan for pre-v3 schemas without hash_to_doc
        for segment_ord in 0..searcher.segment_readers().len() {
            let segment = searcher.segment_reader(segment_ord as u32);
            let alive_bitset = segment.alive_bitset();

            for doc_id in 0..segment.max_doc() {
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

                return apply_doc_enrichment(
                    hit, &doc, &self.fields, &searcher, addr,
                );
            }
        }

        // No matching document found (stale/deleted fact) — return sparse hit unchanged
        hit
    }
}

/// Apply enrichment from a Tantivy document to a sparse hit.
///
/// Only fills empty/zero fields — never overwrites populated data.
/// Preserves the hit's `score` and `bm25_score`.
fn apply_doc_enrichment(
    mut hit: QueryHit,
    doc: &TantivyDocument,
    fields: &ResolvedFields,
    searcher: &Searcher,
    addr: DocAddress,
) -> QueryHit {
    // Overwrite source_path with the real path from the document
    let real_source_path = doc
        .get_first(fields.f_source_path)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if !real_source_path.is_empty() {
        hit.source_path = real_source_path;
    }

    if hit.id.is_empty() {
        hit.id = doc
            .get_first(fields.f_id)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
    }

    if hit.title.is_none() {
        hit.title = doc
            .get_first(fields.f_title)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty());
    }

    if hit.tags.is_empty() {
        hit.tags = doc
            .get_first(fields.f_tags)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .split_whitespace()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();
    }

    if hit.domain_tags.is_empty() {
        hit.domain_tags = doc
            .get_first(fields.f_domain_tags)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .split_whitespace()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();
    }

    if hit.keywords.is_empty() {
        hit.keywords = doc
            .get_first(fields.f_keywords)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .split_whitespace()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();
    }

    if hit.caused_by.is_empty() {
        hit.caused_by = doc
            .get_first(fields.f_caused_by)
            .and_then(|v| v.as_str())
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();
    }

    if hit.causes.is_empty() {
        hit.causes = doc
            .get_first(fields.f_causes)
            .and_then(|v| v.as_str())
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();
    }

    if hit.related.is_empty() {
        hit.related = doc
            .get_first(fields.f_related)
            .and_then(|v| v.as_str())
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();
    }

    if hit.maturity == 1.0 {
        hit.maturity = doc
            .get_first(fields.f_maturity)
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0);
    }

    if hit.access_count == 0 {
        hit.access_count = doc
            .get_first(fields.f_access_count)
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
    }

    if hit.update_count == 0 {
        hit.update_count = doc
            .get_first(fields.f_update_count)
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
    }

    // FAST fields — only fill if at default/zero values
    if hit.importance == 0.0 {
        hit.importance = read_fast_f64(searcher, addr, "importance");
    }
    if hit.confidence == 0.0 {
        hit.confidence = read_fast_f64(searcher, addr, "confidence");
    }
    if hit.recency == 0.0 {
        hit.recency = read_fast_f64(searcher, addr, "recency");
    }
    if hit.fact_type.is_empty() || hit.fact_type == "durable" {
        let fact_type_int = read_fast_u64(searcher, addr, "fact_type_int");
        hit.fact_type = match fact_type_int {
            0 => "durable",
            1 => "state",
            2 => "event",
            _ => "durable",
        }
        .to_string();
    }

    hit
}

/// Build a DocAddressMap from the current Searcher snapshot.
///
/// Scans the `source_path_hash` FAST field across all segments, mapping
/// each hash to its `DocAddress`. The returned map is only valid for the
/// lifetime of the `Searcher` it was built from (DocAddress values encode
/// segment ordinals specific to that snapshot).
///
/// Returns `None` if `f_source_path_hash` is `None` (pre-v3 schema) or
/// if any segment is missing the FAST column.
pub fn build_doc_address_map(
    searcher: &Searcher,
    f_source_path_hash: Option<Field>,
) -> Option<DocAddressMap> {
    let _field = f_source_path_hash?;
    let mut map = DocAddressMap::new();

    for (segment_ord, segment) in searcher.segment_readers().iter().enumerate() {
        let fast = segment.fast_fields();
        let col = match fast.u64("source_path_hash") {
            Ok(c) => c,
            Err(_) => return None, // column missing — pre-v3 index
        };

        let alive = segment.alive_bitset();

        for doc_id in 0..segment.max_doc() {
            if let Some(bitset) = &alive {
                if !bitset.is_alive(doc_id) {
                    continue;
                }
            }
            if let Some(hash) = col.first(doc_id) {
                let addr = DocAddress::new(segment_ord as u32, doc_id);
                map.insert(hash, addr);
            }
        }
    }

    Some(map)
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
        ontology: Option<&OntologyIndex>,
    ) -> Result<Vec<ScoredDoc>, SearchError> {
        let open = self.open()?;
        open.search_with(query_string, options, config, causal_reader, anchor_fact_id, ontology)
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
        ontology: Option<&OntologyIndex>,
    ) -> Result<Vec<ScoredDoc>, SearchError> {
        let f = &self.fields;

        // Ontology-based query expansion (depth-1, OR semantics)
        let effective_query = match ontology {
            Some(ont) if !ont.is_empty() => {
                let tokens: Vec<&str> = query_string.split_whitespace().collect();
                let expanded = ont.expand_tokens(&tokens);
                if expanded.len() > tokens.len() {
                    expanded.join(" ")
                } else {
                    query_string.to_owned()
                }
            }
            _ => query_string.to_owned(),
        };

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
        let query = match query_parser.parse_query(&effective_query) {
            Ok(q) => q,
            Err(_) => {
                let term = tantivy::Term::from_field_text(f.f_title, &effective_query);
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
