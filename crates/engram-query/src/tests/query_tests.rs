use std::path::Path;

use chrono::Utc;
use engram_bulwark::BulwarkHandle;

use crate::cache::ExactCache;
use crate::fuzzy_cache::FuzzyCache;
use crate::{query, QueryError, QueryOptions};

/// Helper: compile a temp dir with specified fixture files and return TempDir.
fn compile_fixtures(fixtures: &[&str]) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let context_tree = tmp.path().join(".brv").join("context-tree");
    std::fs::create_dir_all(&context_tree).unwrap();

    let fixtures_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("engram-compiler")
        .join("tests")
        .join("fixtures");

    for name in fixtures {
        std::fs::copy(fixtures_dir.join(name), context_tree.join(name)).unwrap();
    }

    engram_compiler::compile_context_tree(tmp.path(), true, &BulwarkHandle::new_stub());
    tmp
}

/// Helper: set dirty flag in state file.
fn set_dirty(root: &Path) {
    let state_path = root.join(".brv").join("index").join("state");
    let content = std::fs::read_to_string(&state_path).unwrap();
    let mut state: serde_json::Value = serde_json::from_str(&content).unwrap();
    state["dirty"] = serde_json::Value::Bool(true);
    state["dirty_since"] = serde_json::Value::String(Utc::now().to_rfc3339());
    std::fs::write(&state_path, serde_json::to_string_pretty(&state).unwrap()).unwrap();
}

// --- Test 12: query on missing index returns error ---
#[test]
fn test_query_no_index_returns_error() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cache = ExactCache::new(60);
    let mut fuzzy_cache = FuzzyCache::new(100);
    let bulwark = BulwarkHandle::new_stub();
    let result = query(
        tmp.path(),
        "anything",
        QueryOptions::default(),
        &mut cache,
        &mut fuzzy_cache,
        &bulwark,
    );
    assert!(matches!(result, Err(QueryError::IndexNotFound)));
}

// --- Test 13: query returns results ---
#[test]
fn test_query_returns_results() {
    let tmp = compile_fixtures(&["valid_legacy.md", "valid_engram.md"]);
    let mut cache = ExactCache::new(60);
    let mut fuzzy_cache = FuzzyCache::new(100);
    let bulwark = BulwarkHandle::new_stub();
    let result = query(
        tmp.path(),
        "Legacy",
        QueryOptions::default(),
        &mut cache,
        &mut fuzzy_cache,
        &bulwark,
    )
    .unwrap();
    assert!(!result.hits.is_empty());
}

// --- Test 14: query populates meta ---
#[test]
fn test_query_populates_meta() {
    let tmp = compile_fixtures(&["valid_legacy.md", "valid_engram.md"]);
    let mut cache = ExactCache::new(60);
    let mut fuzzy_cache = FuzzyCache::new(100);
    let bulwark = BulwarkHandle::new_stub();
    let result = query(
        tmp.path(),
        "Legacy",
        QueryOptions::default(),
        &mut cache,
        &mut fuzzy_cache,
        &bulwark,
    )
    .unwrap();
    assert_eq!(result.meta.cache_tier, 2);
    assert!(result.meta.index_generation >= 1);
}

// --- Test 15: second query hits Tier 0 cache ---
#[test]
fn test_query_tier0_cache_hit() {
    let tmp = compile_fixtures(&["valid_legacy.md", "valid_engram.md"]);
    let mut cache = ExactCache::new(60);
    let mut fuzzy_cache = FuzzyCache::new(100);
    let bulwark = BulwarkHandle::new_stub();

    // First query populates cache
    let result1 = query(
        tmp.path(),
        "Legacy",
        QueryOptions::default(),
        &mut cache,
        &mut fuzzy_cache,
        &bulwark,
    )
    .unwrap();
    assert_eq!(result1.meta.cache_tier, 2);

    // Second query hits cache
    let result2 = query(
        tmp.path(),
        "Legacy",
        QueryOptions::default(),
        &mut cache,
        &mut fuzzy_cache,
        &bulwark,
    )
    .unwrap();
    assert_eq!(result2.meta.cache_tier, 0);
}

// --- Test 16: dirty flag skips cache ---
#[test]
fn test_query_dirty_skips_cache() {
    let tmp = compile_fixtures(&["valid_legacy.md", "valid_engram.md"]);
    let mut cache = ExactCache::new(60);
    let mut fuzzy_cache = FuzzyCache::new(100);
    let bulwark = BulwarkHandle::new_stub();

    // Populate cache
    let r1 = query(
        tmp.path(),
        "Legacy",
        QueryOptions::default(),
        &mut cache,
        &mut fuzzy_cache,
        &bulwark,
    )
    .unwrap();
    assert_eq!(r1.meta.cache_tier, 2);

    // Set dirty
    set_dirty(tmp.path());

    // Should bypass cache
    let r2 = query(
        tmp.path(),
        "Legacy",
        QueryOptions::default(),
        &mut cache,
        &mut fuzzy_cache,
        &bulwark,
    )
    .unwrap();
    assert_eq!(r2.meta.cache_tier, 2);
}

// --- Test 17: stale metadata ---
#[test]
fn test_query_stale_metadata() {
    let tmp = compile_fixtures(&["valid_legacy.md"]);
    let mut cache = ExactCache::new(60);
    let mut fuzzy_cache = FuzzyCache::new(100);
    let bulwark = BulwarkHandle::new_stub();

    set_dirty(tmp.path());

    let result = query(
        tmp.path(),
        "Legacy",
        QueryOptions::default(),
        &mut cache,
        &mut fuzzy_cache,
        &bulwark,
    )
    .unwrap();
    assert!(result.meta.stale);
    assert!(result.meta.dirty_since.is_some());
}

// --- Test 18: policy denied blocks query ---
#[test]
fn test_policy_denied_blocks_query() {
    let tmp = compile_fixtures(&["valid_legacy.md"]);
    let mut cache = ExactCache::new(60);
    let mut fuzzy_cache = FuzzyCache::new(100);
    let bulwark = BulwarkHandle::new_denying();
    let result = query(
        tmp.path(),
        "Legacy",
        QueryOptions::default(),
        &mut cache,
        &mut fuzzy_cache,
        &bulwark,
    );
    assert!(matches!(result, Err(QueryError::PolicyDenied(_))));
}

// --- Test 19: query hit fields ---
#[test]
fn test_query_hit_fields() {
    let tmp = compile_fixtures(&["valid_engram.md"]);
    let mut cache = ExactCache::new(60);
    let mut fuzzy_cache = FuzzyCache::new(100);
    let bulwark = BulwarkHandle::new_stub();
    let result = query(
        tmp.path(),
        "Engram",
        QueryOptions::default(),
        &mut cache,
        &mut fuzzy_cache,
        &bulwark,
    )
    .unwrap();
    assert!(!result.hits.is_empty());
    let hit = &result.hits[0];
    assert_eq!(hit.fact_type, "state");
    assert_eq!(hit.confidence, 0.9);
    assert!(hit.domain_tags.contains(&"infra:k8s".to_string()));
}

// --- Test 20: Tier 1 cache hit ---
#[test]
fn test_query_tier1_cache_hit() {
    let tmp = compile_fixtures(&["valid_legacy.md", "valid_engram.md"]);
    let mut cache = ExactCache::new(60);
    let mut fuzzy_cache = FuzzyCache::new(100);
    let bulwark = BulwarkHandle::new_stub();

    // First query populates both caches
    let r1 = query(
        tmp.path(),
        "Rust ownership memory",
        QueryOptions::default(),
        &mut cache,
        &mut fuzzy_cache,
        &bulwark,
    )
    .unwrap();
    assert_eq!(r1.meta.cache_tier, 2);

    // Invalidate Tier 0 so it won't hit
    cache.invalidate_all();

    // Query with same tokens, different order (jaccard = 1.0)
    let r2 = query(
        tmp.path(),
        "memory ownership Rust",
        QueryOptions::default(),
        &mut cache,
        &mut fuzzy_cache,
        &bulwark,
    )
    .unwrap();
    assert_eq!(r2.meta.cache_tier, 1);
}

// --- Test 21: Tier 1 skipped when dirty ---
#[test]
fn test_query_tier1_skipped_when_dirty() {
    let tmp = compile_fixtures(&["valid_legacy.md", "valid_engram.md"]);
    let mut cache = ExactCache::new(60);
    let mut fuzzy_cache = FuzzyCache::new(100);
    let bulwark = BulwarkHandle::new_stub();

    // Populate Tier 1 cache
    let r1 = query(
        tmp.path(),
        "Rust ownership memory",
        QueryOptions::default(),
        &mut cache,
        &mut fuzzy_cache,
        &bulwark,
    )
    .unwrap();
    assert_eq!(r1.meta.cache_tier, 2);

    // Set dirty flag
    set_dirty(tmp.path());

    // Same tokens but dirty — should fall through to Tier 2
    let r2 = query(
        tmp.path(),
        "memory ownership Rust",
        QueryOptions::default(),
        &mut cache,
        &mut fuzzy_cache,
        &bulwark,
    )
    .unwrap();
    assert_eq!(r2.meta.cache_tier, 2);
}
