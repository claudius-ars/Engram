use std::collections::HashSet;

use crate::fuzzy_cache::FuzzyCache;
use crate::result::{QueryHit, QueryMeta, QueryResult};

fn make_result(tag: &str) -> QueryResult {
    QueryResult {
        hits: vec![QueryHit {
            id: tag.to_string(),
            title: Some(format!("Result for {}", tag)),
            source_path: "test.md".to_string(),
            tags: vec![],
            domain_tags: vec![],
            score: 0.9,
            bm25_score: 0.9,
            fact_type: "durable".to_string(),
            confidence: 1.0,
            importance: 1.0,
            recency: 1.0,
            caused_by: vec![],
            causes: vec![],
        }],
        meta: QueryMeta {
            cache_tier: 2,
            stale: false,
            dirty_since: None,
            query_ms: 10,
            total_hits: 1,
            index_generation: 1,
        },
    }
}

fn set_of(words: &[&str]) -> HashSet<String> {
    words.iter().map(|w| w.to_string()).collect()
}

// --- Test 1: tokenize basic ---
#[test]
fn test_tokenize_basic() {
    let tokens = FuzzyCache::tokenize("Rust ownership model");
    assert_eq!(tokens, set_of(&["rust", "ownership", "model"]));
}

// --- Test 2: tokenize punctuation ---
#[test]
fn test_tokenize_punctuation() {
    let tokens = FuzzyCache::tokenize("k8s:infra/compute");
    assert_eq!(tokens, set_of(&["k8s", "infra", "compute"]));
}

// --- Test 3: jaccard identical ---
#[test]
fn test_jaccard_identical() {
    let a = set_of(&["a", "b", "c"]);
    let b = set_of(&["a", "b", "c"]);
    assert_eq!(FuzzyCache::jaccard(&a, &b), 1.0);
}

// --- Test 4: jaccard disjoint ---
#[test]
fn test_jaccard_disjoint() {
    let a = set_of(&["a", "b"]);
    let b = set_of(&["c", "d"]);
    assert_eq!(FuzzyCache::jaccard(&a, &b), 0.0);
}

// --- Test 5: jaccard partial ---
#[test]
fn test_jaccard_partial() {
    let a = set_of(&["rust", "ownership", "model"]);
    let b = set_of(&["rust", "borrow", "checker"]);
    // intersection = {"rust"} = 1
    // union = {"rust","ownership","model","borrow","checker"} = 5
    assert!((FuzzyCache::jaccard(&a, &b) - 0.2).abs() < 1e-10);
}

// --- Test 6: fuzzy cache hit ---
#[test]
fn test_fuzzy_cache_hit() {
    let mut cache = FuzzyCache::new(100);
    cache.insert("Rust ownership model".to_string(), make_result("rom"), 1);

    // "model of Rust ownership" → {"model","of","rust","ownership"}
    // jaccard with {"rust","ownership","model"} = 3/4 = 0.75
    let query_tokens = FuzzyCache::tokenize("model of Rust ownership");
    let result = cache.get(&query_tokens, 0.6, 1, false, 60);
    assert!(result.is_some());
}

// --- Test 7: fuzzy cache miss below threshold ---
#[test]
fn test_fuzzy_cache_miss_below_threshold() {
    let mut cache = FuzzyCache::new(100);
    cache.insert("Rust ownership model".to_string(), make_result("rom"), 1);

    // "Rust borrow checker" → {"rust","borrow","checker"}
    // jaccard with {"rust","ownership","model"} = 1/5 = 0.2
    let query_tokens = FuzzyCache::tokenize("Rust borrow checker");
    let result = cache.get(&query_tokens, 0.6, 1, false, 60);
    assert!(result.is_none());
}

// --- Test 8: fuzzy cache miss dirty ---
#[test]
fn test_fuzzy_cache_miss_dirty() {
    let mut cache = FuzzyCache::new(100);
    cache.insert("Rust ownership model".to_string(), make_result("rom"), 1);

    let query_tokens = FuzzyCache::tokenize("Rust ownership model");
    let result = cache.get(&query_tokens, 0.6, 1, true, 60);
    assert!(result.is_none());
}

// --- Test 9: fuzzy cache miss generation ---
#[test]
fn test_fuzzy_cache_miss_generation() {
    let mut cache = FuzzyCache::new(100);
    cache.insert("Rust ownership model".to_string(), make_result("rom"), 1);

    let query_tokens = FuzzyCache::tokenize("Rust ownership model");
    let result = cache.get(&query_tokens, 0.6, 2, false, 60);
    assert!(result.is_none());
}

// --- Test 10: fuzzy cache eviction ---
#[test]
fn test_fuzzy_cache_eviction() {
    let mut cache = FuzzyCache::new(2);

    cache.insert("alpha bravo charlie".to_string(), make_result("abc"), 1);
    // Small delay to ensure distinct timestamps
    std::thread::sleep(std::time::Duration::from_millis(5));
    cache.insert("delta echo foxtrot".to_string(), make_result("def"), 1);
    std::thread::sleep(std::time::Duration::from_millis(5));
    // This insert evicts "alpha bravo charlie" (oldest)
    cache.insert("golf hotel india".to_string(), make_result("ghi"), 1);

    // "alpha bravo charlie" should be evicted
    let tokens_abc = FuzzyCache::tokenize("alpha bravo charlie");
    assert!(cache.get(&tokens_abc, 0.6, 1, false, 60).is_none());

    // "delta echo foxtrot" should still be present
    let tokens_def = FuzzyCache::tokenize("delta echo foxtrot");
    assert!(cache.get(&tokens_def, 0.6, 1, false, 60).is_some());

    // "golf hotel india" should still be present
    let tokens_ghi = FuzzyCache::tokenize("golf hotel india");
    assert!(cache.get(&tokens_ghi, 0.6, 1, false, 60).is_some());
}

// --- Test 11: fuzzy cache invalidate_all ---
#[test]
fn test_fuzzy_cache_invalidate_all() {
    let mut cache = FuzzyCache::new(100);
    cache.insert("alpha bravo".to_string(), make_result("ab"), 1);
    cache.insert("charlie delta".to_string(), make_result("cd"), 1);
    cache.invalidate_all();

    let tokens_ab = FuzzyCache::tokenize("alpha bravo");
    let tokens_cd = FuzzyCache::tokenize("charlie delta");
    assert!(cache.get(&tokens_ab, 0.6, 1, false, 60).is_none());
    assert!(cache.get(&tokens_cd, 0.6, 1, false, 60).is_none());
}

// --- Test 12: fuzzy cache best match ---
#[test]
fn test_fuzzy_cache_best_match() {
    let mut cache = FuzzyCache::new(100);

    // Entry 1: {"rust", "ownership", "model"} — jaccard with query = 3/4 = 0.75
    cache.insert("Rust ownership model".to_string(), make_result("rom"), 1);

    // Entry 2: {"rust", "ownership", "model", "borrow"} — jaccard with query = 3/4 = 0.75... actually
    // Let's make one that's a closer match.
    // Query will be "rust ownership model memory" → {"rust","ownership","model","memory"}
    // Entry 1: {"rust","ownership","model"} → jaccard = 3/4 = 0.75
    // Entry 2: {"rust","ownership","model","memory","extra"} → jaccard = 4/5 = 0.8

    cache.insert(
        "Rust ownership model memory extra".to_string(),
        make_result("rommen"),
        1,
    );

    let query_tokens = FuzzyCache::tokenize("rust ownership model memory");
    let result = cache.get(&query_tokens, 0.6, 1, false, 60);
    assert!(result.is_some());
    // Should return the entry with higher jaccard (0.8 > 0.75)
    let hit_id = &result.unwrap().hits[0].id;
    assert_eq!(hit_id, "rommen");
}
