use engram_query::{QueryHit, QueryMeta, QueryResult};

use crate::formatter::format_context_block;
use crate::EnrichOptions;

fn make_hit(id: &str, title: Option<&str>, fact_type: &str) -> QueryHit {
    QueryHit {
        id: id.to_string(),
        title: title.map(|s| s.to_string()),
        source_path: format!(".brv/context-tree/{}.md", id),
        tags: vec![],
        domain_tags: vec![],
        score: 0.85,
        bm25_score: 0.9,
        fact_type: fact_type.to_string(),
        confidence: 0.95,
        importance: 1.0,
        recency: 1.0,
        caused_by: vec![],
        causes: vec![],
        keywords: vec![],
        related: vec![],
        maturity: 1.0,
        access_count: 0,
        update_count: 0,
        answer: None,
    }
}

fn make_meta() -> QueryMeta {
    QueryMeta {
        cache_tier: 2,
        stale: false,
        dirty_since: None,
        query_ms: 15,
        total_hits: 2,
        index_generation: 3,
    }
}

// --- Test 1: format with hits ---
#[test]
fn test_format_with_hits() {
    let result = QueryResult {
        hits: vec![
            make_hit("fact-a", Some("Rust Ownership"), "durable"),
            make_hit("fact-b", Some("K8s Scheduling"), "state"),
        ],
        meta: make_meta(),
    };
    let output = format_context_block(&result, &EnrichOptions::default());

    assert!(output.contains("## Engram Context (Auto-Enriched)"));
    assert!(output.contains("<!-- engram:start -->"));
    assert!(output.contains("<!-- engram:end -->"));
    assert!(output.contains("### Rust Ownership"));
    assert!(output.contains("### K8s Scheduling"));
    assert!(output.contains("_Source: .brv/context-tree/fact-a.md_"));
    assert!(output.contains("_Source: .brv/context-tree/fact-b.md_"));
}

// --- Test 2: format no hits ---
#[test]
fn test_format_no_hits() {
    let result = QueryResult {
        hits: vec![],
        meta: make_meta(),
    };
    let output = format_context_block(&result, &EnrichOptions::default());

    assert!(output.contains("_No relevant facts found for this task._"));
    assert!(output.contains("<!-- engram:start -->"));
    assert!(output.contains("<!-- engram:end -->"));
}

// --- Test 3: format with metadata ---
#[test]
fn test_format_with_metadata() {
    let result = QueryResult {
        hits: vec![make_hit("fact-a", Some("Test"), "durable")],
        meta: make_meta(),
    };
    let options = EnrichOptions {
        include_metadata: true,
        ..EnrichOptions::default()
    };
    let output = format_context_block(&result, &options);

    assert!(output.contains("**Score:**"));
    assert!(output.contains("**Tier:** 2"));
    assert!(output.contains("**Gen:** 3"));
}

// --- Test 4: format without metadata ---
#[test]
fn test_format_without_metadata() {
    let result = QueryResult {
        hits: vec![make_hit("fact-a", Some("Test"), "durable")],
        meta: make_meta(),
    };
    let output = format_context_block(&result, &EnrichOptions::default());

    assert!(!output.contains("**Score:**"));
}

// --- Test 5: hit uses id when no title ---
#[test]
fn test_format_hit_uses_id_when_no_title() {
    let result = QueryResult {
        hits: vec![make_hit("infra/k8s", None, "state")],
        meta: make_meta(),
    };
    let output = format_context_block(&result, &EnrichOptions::default());

    assert!(output.contains("### infra/k8s"));
}

// --- Test 6: domain tags shown ---
#[test]
fn test_format_domain_tags_shown() {
    let mut hit = make_hit("fact-a", Some("Test"), "durable");
    hit.domain_tags = vec!["infra:k8s".to_string(), "team:platform".to_string()];
    let result = QueryResult {
        hits: vec![hit],
        meta: make_meta(),
    };
    let output = format_context_block(&result, &EnrichOptions::default());

    assert!(output.contains("**Tags:** infra:k8s, team:platform"));
}

// --- Test 7: no domain tags hidden ---
#[test]
fn test_format_no_domain_tags_hidden() {
    let result = QueryResult {
        hits: vec![make_hit("fact-a", Some("Test"), "durable")],
        meta: make_meta(),
    };
    let output = format_context_block(&result, &EnrichOptions::default());

    assert!(!output.contains("**Tags:**"));
}

// --- Test 8: sentinel stability ---
#[test]
fn test_format_sentinel_stability() {
    let result = QueryResult {
        hits: vec![make_hit("fact-a", Some("Test"), "durable")],
        meta: make_meta(),
    };
    let output = format_context_block(&result, &EnrichOptions::default());

    assert!(output.contains("## Engram Context (Auto-Enriched)"));
    assert!(output.contains("<!-- engram:start -->"));
    assert!(output.contains("<!-- engram:end -->"));
}

// === Phase 2 Prompt 6: QueryHit expansion formatter tests ===

// --- Test 9: keywords shown when non-empty ---
#[test]
fn test_format_keywords_shown() {
    let mut hit = make_hit("fact-a", Some("Test"), "durable");
    hit.keywords = vec!["rust".to_string(), "ownership".to_string()];
    let result = QueryResult {
        hits: vec![hit],
        meta: make_meta(),
    };
    let output = format_context_block(&result, &EnrichOptions::default());
    assert!(output.contains("**Keywords:** rust, ownership"));
}

// --- Test 10: keywords hidden when empty ---
#[test]
fn test_format_keywords_hidden_when_empty() {
    let result = QueryResult {
        hits: vec![make_hit("fact-a", Some("Test"), "durable")],
        meta: make_meta(),
    };
    let output = format_context_block(&result, &EnrichOptions::default());
    assert!(!output.contains("**Keywords:**"));
}

// --- Test 11: related shown when non-empty ---
#[test]
fn test_format_related_shown() {
    let mut hit = make_hit("fact-a", Some("Test"), "durable");
    hit.related = vec!["infra/k8s".to_string(), "auth/sso".to_string()];
    let result = QueryResult {
        hits: vec![hit],
        meta: make_meta(),
    };
    let output = format_context_block(&result, &EnrichOptions::default());
    assert!(output.contains("**Related:** infra/k8s, auth/sso"));
}

// --- Test 12: related hidden when empty ---
#[test]
fn test_format_related_hidden_when_empty() {
    let result = QueryResult {
        hits: vec![make_hit("fact-a", Some("Test"), "durable")],
        meta: make_meta(),
    };
    let output = format_context_block(&result, &EnrichOptions::default());
    assert!(!output.contains("**Related:**"));
}

// --- Test 13: maturity shown when < 1.0 and metadata on ---
#[test]
fn test_format_maturity_shown_when_below_one() {
    let mut hit = make_hit("fact-a", Some("Test"), "durable");
    hit.maturity = 0.6;
    let result = QueryResult {
        hits: vec![hit],
        meta: make_meta(),
    };
    let options = EnrichOptions {
        include_metadata: true,
        ..EnrichOptions::default()
    };
    let output = format_context_block(&result, &options);
    assert!(output.contains("**Maturity:** 0.60"));
}

// --- Test 14: maturity hidden when = 1.0 ---
#[test]
fn test_format_maturity_hidden_when_one() {
    let result = QueryResult {
        hits: vec![make_hit("fact-a", Some("Test"), "durable")],
        meta: make_meta(),
    };
    let options = EnrichOptions {
        include_metadata: true,
        ..EnrichOptions::default()
    };
    let output = format_context_block(&result, &options);
    assert!(!output.contains("**Maturity:**"));
}
