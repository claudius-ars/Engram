pub mod cache;
pub mod fuzzy_cache;
pub mod result;
pub mod searcher;

use std::path::Path;
use std::time::Instant;

use chrono::{DateTime, Utc};
use engram_bulwark::{
    AccessType, AuditEvent, AuditOutcome, BulwarkHandle, PolicyDecision, PolicyRequest,
};

pub use cache::ExactCache;
pub use fuzzy_cache::FuzzyCache;
pub use result::{QueryHit, QueryMeta, QueryResult};
pub use searcher::{BM25Searcher, SearchError};

// Score threshold constants — defined here (not searcher.rs) so they can be
// made configurable per-workspace in a future prompt.
#[allow(dead_code)]
const SCORE_THRESHOLD: f64 = 0.85;
#[allow(dead_code)]
const SCORE_GAP: f64 = 0.1;
const JACCARD_THRESHOLD: f64 = 0.6;

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
            JACCARD_THRESHOLD,
            generation,
            dirty,
            60,
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

    // 6. Tier 2: BM25 search
    let index_dir = root.join(".brv").join("index").join("tantivy");
    let searcher = BM25Searcher::new(&index_dir);
    let start = Instant::now();
    let scored_docs = searcher.search(query_string, &options).map_err(|e| match e {
        SearchError::IndexNotFound(_) => QueryError::IndexNotFound,
        other => QueryError::Search(other),
    })?;
    let query_ms = start.elapsed().as_millis() as u64;

    // 7. Build QueryResult
    let hits: Vec<QueryHit> = scored_docs.into_iter().map(|d| d.hit).collect();

    let meta = QueryMeta {
        cache_tier: 2,
        stale: dirty,
        dirty_since,
        query_ms,
        total_hits: hits.len(),
        index_generation: generation,
    };

    let result = QueryResult { hits, meta };

    // 8. Insert into Tier 0 and Tier 1 caches
    cache.insert(fingerprint, result.clone(), generation);
    fuzzy_cache.insert(query_string.to_string(), result.clone(), generation);

    // 9. Bulwark audit
    bulwark.audit(AuditEvent {
        request,
        decision: PolicyDecision::Allow,
        outcome: AuditOutcome::Success,
        timestamp: Utc::now(),
    });

    // 10. Return
    Ok(result)
}

#[cfg(test)]
mod tests;
