//! Causal query tier: trigger detection, pattern classification, traversal,
//! and result merging for causal graph queries.

use std::collections::HashSet;

use engram_core::temporal::fnv1a_64;

use crate::causal_reader::{CausalReader, TraversalDirection, CAUSAL_DECAY_BASE};
use crate::result::QueryHit;

/// Cache tier value for causal query results.
/// 24 = causal tier. Lower than temporal (25) — temporal wins on deduplication collision.
pub const CACHE_TIER_CAUSAL: u8 = 24;

/// Causal signal phrases. All comparisons are case-insensitive substring matches.
const CAUSAL_SIGNALS: &[&str] = &[
    "caused by",
    "depends on",
    "enables",
    "led to",
    "because",
    "therefore",
    "chain",
    "upstream",
    "downstream",
    "root cause",
    "consequence of",
];

/// Returns true if the query contains a causal signal phrase (case-insensitive substring).
pub fn is_causal_query(query: &str) -> bool {
    let lower = query.to_lowercase();
    CAUSAL_SIGNALS.iter().any(|&signal| lower.contains(signal))
}

/// Causal query pattern, dispatched by signal word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CausalQueryPattern {
    /// "what caused X" / "root cause" / "upstream" → backward traversal
    Backward,
    /// "enables" / "downstream" / "consequence of" → forward traversal
    Forward,
    /// "chain" / "depends on" / "led to" / "because" / "therefore" → shortest path per candidate
    Chain,
}

/// Classify a causal query into a traversal pattern.
pub fn classify_causal_query(query: &str) -> CausalQueryPattern {
    let lower = query.to_lowercase();

    // Backward: caused by, root cause, upstream
    if lower.contains("caused by")
        || lower.contains("root cause")
        || lower.contains("upstream")
    {
        return CausalQueryPattern::Backward;
    }

    // Forward: enables, downstream, consequence of
    if lower.contains("enables")
        || lower.contains("downstream")
        || lower.contains("consequence of")
    {
        return CausalQueryPattern::Forward;
    }

    // Chain: chain, depends on, led to, because, therefore
    CausalQueryPattern::Chain
}

/// Build a `QueryHit` from a causal graph node.
fn causal_node_to_query_hit(
    reader: &CausalReader,
    node_index: u32,
    score: f64,
) -> QueryHit {
    let fact_id = reader
        .node_fact_id(node_index)
        .unwrap_or("")
        .to_string();
    let source_path_hash = reader
        .node_source_path_hash(node_index)
        .unwrap_or(0);

    QueryHit {
        id: fact_id.clone(),
        title: None,
        source_path: format!("<causal:{:016x}>", source_path_hash),
        tags: vec![],
        domain_tags: vec![],
        score,
        bm25_score: 0.0,
        fact_type: String::new(),
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
    }
}

/// Run causal traversal from the anchor, producing additional `QueryHit`s.
///
/// `anchor_fact_id` is the fact ID of the top BM25 result.
/// `bm25_hits` are the existing BM25 results (for chain-mode shortest path).
pub fn causal_traversal(
    reader: &CausalReader,
    anchor_fact_id: &str,
    pattern: &CausalQueryPattern,
    max_hops: u8,
    bm25_hits: &[QueryHit],
) -> Vec<QueryHit> {
    let anchor_idx = match reader.fact_id_to_node(anchor_fact_id) {
        Some(idx) => idx,
        None => return Vec::new(),
    };

    match pattern {
        CausalQueryPattern::Backward => {
            let reachable = reader.reachable_within(anchor_idx, max_hops, TraversalDirection::Backward);
            reachable
                .into_iter()
                .map(|(node, hops)| {
                    let score = CAUSAL_DECAY_BASE.powi(hops as i32);
                    causal_node_to_query_hit(reader, node, score)
                })
                .collect()
        }
        CausalQueryPattern::Forward => {
            let reachable = reader.reachable_within(anchor_idx, max_hops, TraversalDirection::Forward);
            reachable
                .into_iter()
                .map(|(node, hops)| {
                    let score = CAUSAL_DECAY_BASE.powi(hops as i32);
                    causal_node_to_query_hit(reader, node, score)
                })
                .collect()
        }
        CausalQueryPattern::Chain => {
            // For each BM25 hit, find shortest path from anchor
            let mut hits = Vec::new();
            for bm25_hit in bm25_hits {
                let candidate_id = &bm25_hit.id;
                if candidate_id.is_empty() {
                    continue;
                }
                let candidate_idx = match reader.fact_id_to_node(candidate_id) {
                    Some(idx) => idx,
                    None => continue,
                };
                if candidate_idx == anchor_idx {
                    continue;
                }
                if let Some(path) = reader.shortest_path(anchor_idx, candidate_idx, max_hops) {
                    let hops = (path.len() - 1) as u8;
                    let score = CAUSAL_DECAY_BASE.powi(hops as i32);
                    hits.push(causal_node_to_query_hit(reader, candidate_idx, score));
                }
            }
            hits
        }
    }
}

/// Merge causal hits with BM25 hits, deduplicating by source_path_hash.
/// BM25 result wins on collision (keeps higher-fidelity data). Causal-only
/// results are appended after BM25 results.
///
/// Higher cache_tier wins — temporal (25) > causal (24) > BM25 (2).
/// Temporal merge runs after causal merge in the pipeline, so temporal
/// always overwrites causal duplicates.
pub fn merge_causal_and_bm25(
    causal_hits: Vec<QueryHit>,
    bm25_hits: Vec<QueryHit>,
) -> Vec<QueryHit> {
    let bm25_hashes: HashSet<u64> = bm25_hits
        .iter()
        .map(|h| fnv1a_64(h.source_path.as_bytes()))
        .collect();

    let mut merged = bm25_hits;

    for hit in causal_hits {
        let hash = fnv1a_64(hit.source_path.as_bytes());
        if !bm25_hashes.contains(&hash) {
            merged.push(hit);
        }
    }

    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── is_causal_query tests ──────────────────────────────────────────

    #[test]
    fn test_is_causal_query_caused_by() {
        assert!(is_causal_query("what caused by the outage"));
    }

    #[test]
    fn test_is_causal_query_depends_on() {
        assert!(is_causal_query("this depends on the auth service"));
    }

    #[test]
    fn test_is_causal_query_enables() {
        assert!(is_causal_query("the cache layer enables fast reads"));
    }

    #[test]
    fn test_is_causal_query_led_to() {
        assert!(is_causal_query("the migration led to data loss"));
    }

    #[test]
    fn test_is_causal_query_because() {
        assert!(is_causal_query("it failed because of a timeout"));
    }

    #[test]
    fn test_is_causal_query_therefore() {
        assert!(is_causal_query("therefore we need a new approach"));
    }

    #[test]
    fn test_is_causal_query_chain() {
        assert!(is_causal_query("show me the causal chain"));
    }

    #[test]
    fn test_is_causal_query_upstream() {
        assert!(is_causal_query("what is upstream of this service"));
    }

    #[test]
    fn test_is_causal_query_downstream() {
        assert!(is_causal_query("downstream effects of the change"));
    }

    #[test]
    fn test_is_causal_query_root_cause() {
        assert!(is_causal_query("what is the root cause"));
    }

    #[test]
    fn test_is_causal_query_consequence_of() {
        assert!(is_causal_query("this is a consequence of the redesign"));
    }

    #[test]
    fn test_is_causal_query_case_insensitive() {
        assert!(is_causal_query("What CAUSED BY the incident"));
        assert!(is_causal_query("ROOT CAUSE analysis"));
    }

    #[test]
    fn test_is_causal_query_negative() {
        assert!(!is_causal_query("what is the retry policy"));
        assert!(!is_causal_query("explain authentication flow"));
        assert!(!is_causal_query("how does the cache work"));
    }

    #[test]
    fn test_is_causal_query_no_false_positive_led() {
        // "led to" should match, but "I led the team" should also match
        // because "led to" is not present — it's a substring check
        assert!(!is_causal_query("I led the team"));
    }

    // ─── classify_causal_query tests ────────────────────────────────────

    #[test]
    fn test_classify_backward() {
        assert_eq!(
            classify_causal_query("what caused by the outage"),
            CausalQueryPattern::Backward
        );
        assert_eq!(
            classify_causal_query("root cause of the failure"),
            CausalQueryPattern::Backward
        );
        assert_eq!(
            classify_causal_query("what is upstream of auth"),
            CausalQueryPattern::Backward
        );
    }

    #[test]
    fn test_classify_forward() {
        assert_eq!(
            classify_causal_query("what enables fast reads"),
            CausalQueryPattern::Forward
        );
        assert_eq!(
            classify_causal_query("downstream effects"),
            CausalQueryPattern::Forward
        );
        assert_eq!(
            classify_causal_query("consequence of the change"),
            CausalQueryPattern::Forward
        );
    }

    #[test]
    fn test_classify_chain() {
        assert_eq!(
            classify_causal_query("show me the chain"),
            CausalQueryPattern::Chain
        );
        assert_eq!(
            classify_causal_query("this depends on auth"),
            CausalQueryPattern::Chain
        );
        assert_eq!(
            classify_causal_query("the migration led to data loss"),
            CausalQueryPattern::Chain
        );
        assert_eq!(
            classify_causal_query("failed because of timeout"),
            CausalQueryPattern::Chain
        );
        assert_eq!(
            classify_causal_query("therefore we need a fix"),
            CausalQueryPattern::Chain
        );
    }

    // ─── merge_causal_and_bm25 tests ────────────────────────────────────

    fn make_hit(id: &str, source_path: &str, score: f64) -> QueryHit {
        QueryHit {
            id: id.to_string(),
            title: None,
            source_path: source_path.to_string(),
            tags: vec![],
            domain_tags: vec![],
            score,
            bm25_score: 0.0,
            fact_type: String::new(),
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
        }
    }

    #[test]
    fn test_merge_dedup_bm25_wins() {
        let bm25 = vec![make_hit("a", "shared.md", 0.9)];
        let causal = vec![make_hit("a", "shared.md", 0.5)];

        let merged = merge_causal_and_bm25(causal, bm25);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].score, 0.9); // BM25 score kept
    }

    #[test]
    fn test_merge_causal_only_appended() {
        let bm25 = vec![make_hit("a", "a.md", 0.9)];
        let causal = vec![make_hit("b", "b.md", 0.7)];

        let merged = merge_causal_and_bm25(causal, bm25);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].source_path, "a.md"); // BM25 first
        assert_eq!(merged[1].source_path, "b.md"); // causal appended
    }

    #[test]
    fn test_merge_empty_causal() {
        let bm25 = vec![make_hit("a", "a.md", 0.9)];
        let merged = merge_causal_and_bm25(vec![], bm25);
        assert_eq!(merged.len(), 1);
    }

    #[test]
    fn test_merge_empty_bm25() {
        let causal = vec![make_hit("a", "a.md", 0.7)];
        let merged = merge_causal_and_bm25(causal, vec![]);
        assert_eq!(merged.len(), 1);
    }

    // ─── empty reader produces no causal hits ───────────────────────────

    #[test]
    fn test_causal_traversal_empty_reader() {
        let reader = CausalReader::empty();
        let hits = causal_traversal(
            &reader,
            "anything",
            &CausalQueryPattern::Backward,
            3,
            &[],
        );
        assert!(hits.is_empty());
    }
}
