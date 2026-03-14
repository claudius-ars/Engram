use crate::cache::ExactCache;
use crate::result::{QueryHit, QueryMeta, QueryResult};

fn make_result() -> QueryResult {
    QueryResult {
        hits: vec![QueryHit {
            id: "test".to_string(),
            title: Some("Test".to_string()),
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
            keywords: vec![],
            related: vec![],
            maturity: 1.0,
            access_count: 0,
            update_count: 0,
            answer: None,
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

// --- Test 1: cache miss on empty cache ---
#[test]
fn test_cache_miss_empty() {
    let cache = ExactCache::new(60);
    assert!(cache.get("abc123", 1, false).is_none());
}

// --- Test 2: cache hit ---
#[test]
fn test_cache_hit() {
    let mut cache = ExactCache::new(60);
    cache.insert("abc123".to_string(), make_result(), 1);
    assert!(cache.get("abc123", 1, false).is_some());
}

// --- Test 3: cache miss when dirty ---
#[test]
fn test_cache_miss_dirty() {
    let mut cache = ExactCache::new(60);
    cache.insert("abc123".to_string(), make_result(), 1);
    assert!(cache.get("abc123", 1, true).is_none());
}

// --- Test 4: cache miss when generation changed ---
#[test]
fn test_cache_miss_generation_changed() {
    let mut cache = ExactCache::new(60);
    cache.insert("abc123".to_string(), make_result(), 1);
    assert!(cache.get("abc123", 2, false).is_none());
}

// --- Test 5: cache miss when expired ---
#[test]
fn test_cache_miss_expired() {
    let mut cache = ExactCache::new(0);
    cache.insert("abc123".to_string(), make_result(), 1);
    // TTL is 0 seconds, so it's immediately expired
    assert!(cache.get("abc123", 1, false).is_none());
}

// --- Test 6: invalidate_all clears all entries ---
#[test]
fn test_cache_invalidate_all() {
    let mut cache = ExactCache::new(60);
    cache.insert("key1".to_string(), make_result(), 1);
    cache.insert("key2".to_string(), make_result(), 1);
    cache.invalidate_all();
    assert!(cache.get("key1", 1, false).is_none());
    assert!(cache.get("key2", 1, false).is_none());
}
