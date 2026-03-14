use std::collections::HashSet;

use engram_core::temporal::{
    fnv1a_64, TemporalRecord, FACT_TYPE_DURABLE, FACT_TYPE_EVENT, FACT_TYPE_STATE,
};

use crate::result::QueryHit;

/// Cache tier value for Tier 2.5 (temporal query).
pub const CACHE_TIER_TEMPORAL: u8 = 25;

/// Temporal signal words. Multi-word entries use substring match;
/// single-word entries require word boundary matching.
pub const TEMPORAL_SIGNAL_WORDS: &[&str] = &[
    "active",
    "current",
    "currently",
    "latest",
    "now",
    "since",
    "before",
    "after",
    "changed",
    "history",
    "was",
    "still",
    "when did",
    "as of",
    "recent",
    "recently",
];

/// Returns true if the query string contains a temporal signal word.
///
/// Single-word signals require word boundaries (e.g. "was" must not
/// match inside "password"). Multi-word signals use substring match.
pub fn has_temporal_signal(query: &str) -> bool {
    let lower = query.to_lowercase();
    TEMPORAL_SIGNAL_WORDS.iter().any(|&signal| {
        if signal.contains(' ') {
            lower.contains(signal)
        } else {
            lower.split_whitespace().any(|word| {
                let word = word.trim_matches(|c: char| c.is_ascii_punctuation());
                word == signal
            })
        }
    })
}

/// Temporal query pattern classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemporalQueryPattern {
    /// "What is the current state of X?"
    CurrentState,
    /// "What changed since <timestamp>?" — parsed unix seconds, or i64::MIN if unparseable
    SinceTimestamp(i64),
    /// "Show me the history of X"
    EventHistory,
}

/// Classify a temporal query into one of three patterns.
pub fn classify_temporal_query(query: &str) -> TemporalQueryPattern {
    let lower = query.to_lowercase();

    // SinceTimestamp: detect "since" followed by something year-like
    if lower.contains("since") {
        let ts = extract_since_timestamp(&lower);
        return TemporalQueryPattern::SinceTimestamp(ts);
    }

    // EventHistory: detect history/event keywords
    if lower.contains("history")
        || lower.contains("when did")
        || lower.contains("what happened")
        || lower.contains("changed")
    {
        return TemporalQueryPattern::EventHistory;
    }

    // Default for temporal-triggered queries: current state
    TemporalQueryPattern::CurrentState
}

/// Attempt to extract a unix timestamp from a "since ..." clause.
/// Looks for a 4-digit year and converts to Jan 1 of that year UTC.
/// Returns `i64::MIN` if no parseable timestamp is found.
pub fn extract_since_timestamp(query: &str) -> i64 {
    // Look for a 4-digit number that could be a year
    for word in query.split_whitespace() {
        let word = word.trim_matches(|c: char| c.is_ascii_punctuation());
        if word.len() == 4 {
            if let Ok(year) = word.parse::<i32>() {
                if (2000..=2100).contains(&year) {
                    // Convert to Jan 1 of that year UTC
                    if let Some(dt) = chrono::NaiveDate::from_ymd_opt(year, 1, 1) {
                        let ts = dt
                            .and_hms_opt(0, 0, 0)
                            .unwrap()
                            .and_utc()
                            .timestamp();
                        return ts;
                    }
                }
            }
        }
    }
    i64::MIN
}

/// Convert a TemporalRecord to a sparse QueryHit.
///
/// The resulting hit has minimal fields (source_path as `<temporal:hash>`,
/// no title/tags/keywords). Call `OpenIndex::enrich_hit()` to
/// populate the full field set from the Tantivy index.
pub fn temporal_record_to_query_hit(record: &TemporalRecord) -> QueryHit {
    let fact_type = match record.fact_type {
        FACT_TYPE_DURABLE => "durable",
        FACT_TYPE_STATE => "state",
        FACT_TYPE_EVENT => "event",
        _ => "durable",
    }
    .to_string();

    QueryHit {
        id: String::new(),
        title: None,
        source_path: format!("<temporal:{:016x}>", record.source_path_hash),
        tags: vec![],
        domain_tags: vec![],
        score: 1.0, // temporal hits are authoritative
        bm25_score: 0.0,
        fact_type,
        confidence: 0.0,
        importance: 0.0,
        recency: 0.0,
        caused_by: vec![],
        causes: vec![],
        keywords: vec![],
        related: vec![],
        maturity: 0.0,
        access_count: 0,
        update_count: 0,
        answer: None,
    }
}

/// Boost added to BM25 scores for facts that also appear in the
/// temporal tier, ensuring they rank above pure-BM25 hits while
/// preserving relevance ordering within the temporal block.
const TEMPORAL_BOOST: f64 = 2.0;

/// Merge temporal hits (Tier 2.5) and BM25 hits (Tier 2), deduplicating
/// by source_path_hash. Temporal hits are re-scored using their BM25
/// relevance plus a boost so they rank above pure-BM25 hits but are
/// ordered by query relevance within the temporal block.
pub fn merge_temporal_and_bm25(
    mut temporal_hits: Vec<QueryHit>,
    temporal_hashes: &HashSet<u64>,
    bm25_hits: Vec<QueryHit>,
) -> Vec<QueryHit> {
    // Build a lookup of BM25 scores by source_path hash
    let bm25_scores: std::collections::HashMap<u64, f64> = bm25_hits
        .iter()
        .map(|hit| (fnv1a_64(hit.source_path.as_bytes()), hit.score))
        .collect();

    // Re-score temporal hits: BM25 relevance + boost
    for hit in &mut temporal_hits {
        let hash = fnv1a_64(hit.source_path.as_bytes());
        let bm25 = bm25_scores.get(&hash).copied().unwrap_or(0.0);
        hit.score = bm25 + TEMPORAL_BOOST;
    }

    // Sort temporal hits by score descending (most relevant first)
    temporal_hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

    let mut merged = Vec::with_capacity(temporal_hits.len() + bm25_hits.len());
    merged.extend(temporal_hits);

    for hit in bm25_hits {
        let hash = fnv1a_64(hit.source_path.as_bytes());
        if !temporal_hashes.contains(&hash) {
            merged.push(hit);
        }
    }

    merged
}
