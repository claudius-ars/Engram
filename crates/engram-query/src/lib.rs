pub mod cache;
pub mod causal_query;
pub mod causal_reader;
pub mod fuzzy_cache;
pub mod result;
pub mod searcher;
pub mod temporal_query;
pub mod temporal_reader;

use std::collections::HashSet;
use std::path::Path;
use std::time::Instant;

use chrono::{DateTime, Utc};
use engram_bulwark::{
    AccessType, AuditEvent, AuditOutcome, BulwarkHandle, PolicyDecision, PolicyRequest,
};
use engram_core::WorkspaceConfig;

use causal_query::{
    causal_traversal, classify_causal_query, is_causal_query, merge_causal_and_bm25,
};
use causal_reader::CausalReader;
use temporal_query::{
    classify_temporal_query, has_temporal_signal, merge_temporal_and_bm25,
    temporal_record_to_query_hit,
};
use temporal_reader::TemporalReader;

pub use cache::ExactCache;
pub use causal_query::CACHE_TIER_CAUSAL;
pub use fuzzy_cache::FuzzyCache;
pub use result::{QueryHit, QueryMeta, QueryResult};
pub use searcher::{BM25Searcher, OpenIndex, SearchError};
pub use temporal_query::CACHE_TIER_TEMPORAL;

#[derive(Debug, Clone)]
pub struct QueryOptions {
    pub max_results: usize,
    pub min_score: f64,
}

impl Default for QueryOptions {
    fn default() -> Self {
        QueryOptions {
            max_results: 10,
            min_score: 0.0,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum QueryError {
    #[error("index not found — run engram compile first")]
    IndexNotFound,

    #[error("search error: {0}")]
    Search(#[from] SearchError),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("policy denied: {0}")]
    PolicyDenied(String),
}

/// Read the index state file. Returns (dirty, dirty_since, generation).
fn read_index_state(root: &Path) -> (bool, Option<DateTime<Utc>>, u64) {
    let state_path = root.join(".brv").join("index").join("state");
    if !state_path.exists() {
        return (false, None, 0);
    }

    let content = match std::fs::read_to_string(&state_path) {
        Ok(c) => c,
        Err(_) => return (false, None, 0),
    };

    #[derive(serde::Deserialize)]
    struct StateSummary {
        dirty: bool,
        dirty_since: Option<DateTime<Utc>>,
        generation: u64,
    }

    match serde_json::from_str::<StateSummary>(&content) {
        Ok(s) => (s.dirty, s.dirty_since, s.generation),
        Err(_) => (false, None, 0),
    }
}

pub fn query(
    root: &Path,
    query_string: &str,
    options: QueryOptions,
    cache: &mut ExactCache,
    fuzzy_cache: &mut FuzzyCache,
    bulwark: &BulwarkHandle,
    config: &WorkspaceConfig,
) -> Result<QueryResult, QueryError> {
    // 1. Bulwark policy check
    let request = PolicyRequest {
        access_type: AccessType::Read,
        fact_id: None,
        agent_id: None,
        operation: "query".to_string(),
    };
    if let PolicyDecision::Deny { reason } = bulwark.check(&request) {
        return Err(QueryError::PolicyDenied(reason));
    }

    // 2. Read state
    let (dirty, dirty_since, generation) = read_index_state(root);

    // 3. Compute MD5 fingerprint
    let fingerprint = format!("{:x}", md5::compute(query_string.as_bytes()));

    // 4. Tier 0: Exact cache
    if let Some(cached) = cache.get(&fingerprint, generation, dirty) {
        let mut result = cached.clone();
        result.meta.cache_tier = 0;
        bulwark.audit(AuditEvent {
            request,
            decision: PolicyDecision::Allow,
            outcome: AuditOutcome::Success,
            timestamp: Utc::now(),
        });
        return Ok(result);
    }

    // Cache invalidation contract (Phase 1):
    // Both Tier 0 and Tier 1 are invalidated by two mechanisms:
    //   1. dirty flag: bypassed immediately when state.dirty == true
    //   2. generation counter: entries from prior index generations
    //      are rejected at get() even if not dirty
    // Phase 4 adds explicit invalidate_all() calls on curate events.

    // 5. Tier 1: Fuzzy cache
    if !dirty {
        let query_tokens = FuzzyCache::tokenize(query_string);
        if let Some(cached) = fuzzy_cache.get(
            &query_tokens,
            config.jaccard_threshold,
            generation,
            dirty,
            config.exact_cache_ttl_secs,
        ) {
            let mut result = cached.clone();
            result.meta.cache_tier = 1;
            bulwark.audit(AuditEvent {
                request: request.clone(),
                decision: PolicyDecision::Allow,
                outcome: AuditOutcome::Success,
                timestamp: Utc::now(),
            });
            return Ok(result);
        }
    }

    // 6. Load CausalReader (once per query session, fallback to empty)
    let parent_index_dir = root.join(".brv").join("index");
    let causal_reader = match CausalReader::load(&parent_index_dir, generation) {
        Ok(r) => r,
        Err(_) => CausalReader::empty(),
    };

    // 7. Open Tantivy index (once per query session)
    let index_dir = parent_index_dir.join("tantivy");
    let bm25_searcher = BM25Searcher::new(&index_dir);
    let start = Instant::now();
    let open_index = bm25_searcher.open().map_err(|e| match e {
        SearchError::IndexNotFound(_) => QueryError::IndexNotFound,
        other => QueryError::Search(other),
    })?;

    // 8. Tier 2: BM25 search (first pass: no anchor yet → causal_adj = 1.0 for events)
    let scored_docs = open_index
        .search_with(query_string, &options, config, &causal_reader, None)
        .map_err(|e| match e {
            SearchError::IndexNotFound(_) => QueryError::IndexNotFound,
            other => QueryError::Search(other),
        })?;

    let bm25_hits: Vec<QueryHit> = scored_docs.into_iter().map(|d| d.hit).collect();

    // 9. Determine anchor for causal scoring (top BM25 result)
    let anchor_fact_id: Option<String> = bm25_hits.first().map(|h| h.id.clone());

    // 10. Re-score with causal anchor if we have one and the graph is non-empty
    let bm25_hits = if anchor_fact_id.is_some() && causal_reader.node_count() > 0 {
        let anchor = anchor_fact_id.as_deref().unwrap();
        match open_index.search_with(query_string, &options, config, &causal_reader, Some(anchor)) {
            Ok(docs) => docs.into_iter().map(|d| d.hit).collect(),
            Err(_) => bm25_hits,
        }
    } else {
        bm25_hits
    };

    // 11. Tier 2.5: Causal query (only if causal signal detected)
    let (hits_after_causal, cache_tier) = if is_causal_query(query_string) {
        if let Some(anchor) = &anchor_fact_id {
            let pattern = classify_causal_query(query_string);
            let causal_hits = causal_traversal(
                &causal_reader,
                anchor,
                &pattern,
                config.causal_max_hops,
                &bm25_hits,
            );
            if !causal_hits.is_empty() {
                let merged = merge_causal_and_bm25(causal_hits, bm25_hits);
                (merged, CACHE_TIER_CAUSAL)
            } else {
                (bm25_hits, 2)
            }
        } else {
            (bm25_hits, 2)
        }
    } else {
        (bm25_hits, 2)
    };

    // 12. Tier 2.5b: Temporal query (only if temporal signal detected)
    let (hits, cache_tier) = if has_temporal_signal(query_string) {
        if let Ok(Some(reader)) = TemporalReader::load(&parent_index_dir) {
            let pattern = classify_temporal_query(query_string);
            let now_ts = chrono::Utc::now().timestamp();
            let temporal_records = reader.tier2_5_search(&pattern, now_ts, generation);

            if !temporal_records.is_empty() {
                let temporal_hashes: HashSet<u64> =
                    temporal_records.iter().map(|r| r.source_path_hash).collect();

                // Build sparse temporal hits, then enrich from Tantivy index
                let temporal_hits: Vec<QueryHit> = temporal_records
                    .iter()
                    .map(|r| temporal_record_to_query_hit(r))
                    .map(|hit| open_index.enrich_temporal_hit(hit))
                    .collect();

                let merged = merge_temporal_and_bm25(
                    temporal_hits,
                    &temporal_hashes,
                    hits_after_causal,
                );
                (merged, CACHE_TIER_TEMPORAL)
            } else {
                (hits_after_causal, cache_tier)
            }
        } else {
            (hits_after_causal, cache_tier)
        }
    } else {
        (hits_after_causal, cache_tier)
    };

    let query_ms = start.elapsed().as_millis() as u64;

    // 13. Build QueryResult
    let meta = QueryMeta {
        cache_tier,
        stale: dirty,
        dirty_since,
        query_ms,
        total_hits: hits.len(),
        index_generation: generation,
    };

    let result = QueryResult { hits, meta };

    // 14. Insert into Tier 0 and Tier 1 caches
    cache.insert(fingerprint, result.clone(), generation);
    fuzzy_cache.insert(query_string.to_string(), result.clone(), generation);

    // 15. Bulwark audit
    bulwark.audit(AuditEvent {
        request,
        decision: PolicyDecision::Allow,
        outcome: AuditOutcome::Success,
        timestamp: Utc::now(),
    });

    // 16. Return
    Ok(result)
}

#[cfg(test)]
mod tests;
