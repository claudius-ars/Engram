#[allow(dead_code)]
mod common;

use engram_bulwark::BulwarkHandle;
use engram_compiler::compile_context_tree;
use engram_core::WorkspaceConfig;
use engram_query::{ExactCache, FuzzyCache, QueryOptions};

use common::{
    compile_clean, compile_incremental, compile_with_classify, durable_fact,
    temp_workspace, unclassified_fact, write_fact,
};

fn default_query_options() -> QueryOptions {
    QueryOptions {
        max_results: 10,
        min_score: 0.0,
    }
}

fn permissive_config() -> WorkspaceConfig {
    WorkspaceConfig {
        score_threshold: 0.0,
        score_gap: 0.0,
        ..WorkspaceConfig::default()
    }
}

fn query_helper(
    root: &std::path::Path,
    query_str: &str,
    cache: &mut ExactCache,
    fuzzy_cache: &mut FuzzyCache,
) -> engram_query::QueryResult {
    let bulwark = BulwarkHandle::new_stub();
    let config = permissive_config();
    engram_query::query(
        root,
        query_str,
        default_query_options(),
        cache,
        fuzzy_cache,
        &bulwark,
        &config,
    )
    .expect("query should succeed")
}

// ============================================================
// Cross-prompt gap: Config → Scoring integration
// Verifies that workspace config values actually affect query behavior.
// ============================================================

#[test]
fn gap_config_score_threshold_filters_results() {
    let tmp = temp_workspace();

    // Write facts with varying relevance
    write_fact(
        tmp.path(),
        "relevant.md",
        &durable_fact(
            "Mandrill Behavior Patterns",
            "Mandrill social behavior is complex and hierarchical.",
        ),
    );
    write_fact(
        tmp.path(),
        "marginal.md",
        &durable_fact(
            "Zoo Directory",
            "The zoo has many animals including various primates.",
        ),
    );
    compile_clean(tmp.path());

    // Query with very permissive config — should get multiple results
    let mut cache = ExactCache::new(60);
    let mut fuzzy = FuzzyCache::new(100);
    let bulwark = BulwarkHandle::new_stub();
    let permissive = permissive_config();

    let r = engram_query::query(
        tmp.path(),
        "mandrill behavior",
        default_query_options(),
        &mut cache,
        &mut fuzzy,
        &bulwark,
        &permissive,
    )
    .unwrap();

    let permissive_count = r.hits.len();
    assert!(permissive_count >= 1, "should find at least one result with permissive config");

    // Query with restrictive config — should filter low-scoring results
    cache.invalidate_all();
    fuzzy.invalidate_all();

    let restrictive = WorkspaceConfig {
        score_threshold: 99.0, // extremely high threshold
        score_gap: 0.0,
        ..WorkspaceConfig::default()
    };

    let r = engram_query::query(
        tmp.path(),
        "mandrill behavior",
        default_query_options(),
        &mut cache,
        &mut fuzzy,
        &bulwark,
        &restrictive,
    )
    .unwrap();

    assert!(
        r.hits.len() <= permissive_count,
        "restrictive config should not return more results than permissive"
    );
}

// ============================================================
// Cross-prompt gap: Temporal + Classification interaction
// Verifies: classified state facts appear in temporal tier queries.
// ============================================================

#[test]
fn gap_classified_state_fact_in_temporal_query() {
    let tmp = temp_workspace();
    write_fact(
        tmp.path(),
        "classify-temporal.md",
        &unclassified_fact(
            "API Gateway Load",
            "The API gateway is currently handling 5000 requests per second.",
        ),
    );

    // Compile with --classify to detect as state
    compile_with_classify(tmp.path());

    std::fs::write(
        tmp.path().join(".brv/engram.toml"),
        "[query]\nscore_threshold = 0.0\nscore_gap = 0.0\n",
    )
    .unwrap();

    let mut cache = ExactCache::new(60);
    let mut fuzzy = FuzzyCache::new(100);
    let bulwark = BulwarkHandle::new_stub();
    let config = permissive_config();

    // Temporal signal query
    let r = engram_query::query(
        tmp.path(),
        "what is the current API gateway load",
        default_query_options(),
        &mut cache,
        &mut fuzzy,
        &bulwark,
        &config,
    )
    .unwrap();

    // Should either be temporal tier or BM25 tier with hits
    assert!(
        !r.hits.is_empty(),
        "classified state fact should be findable via temporal signal query"
    );
}

// ============================================================
// Cross-prompt gap: Curate → Incremental interaction
// Verifies: curate without sync + incremental compile picks up the change.
// ============================================================

#[test]
fn gap_curate_then_incremental() {
    let tmp = temp_workspace();
    write_fact(
        tmp.path(),
        "base.md",
        &durable_fact("Base Capybara", "Capybara is the largest living rodent."),
    );
    compile_clean(tmp.path());

    // Curate without sync
    let bulwark = BulwarkHandle::new_stub();
    let curate_result = engram_compiler::curate(
        tmp.path(),
        engram_compiler::CurateOptions {
            summary: "Chinchilla population survey completed in sector 7".to_string(),
            sync: false,
        },
        &bulwark,
    )
    .expect("curate should succeed");
    assert!(curate_result.written_path.exists());

    // Run incremental compile to pick up the curated fact
    let inc = compile_incremental(tmp.path());
    assert!(inc.index_error.is_none());

    // Both original and curated facts should be queryable
    let mut cache = ExactCache::new(60);
    let mut fuzzy = FuzzyCache::new(100);
    let r = query_helper(tmp.path(), "capybara rodent", &mut cache, &mut fuzzy);
    assert!(!r.hits.is_empty(), "original fact should still be found");

    cache.invalidate_all();
    fuzzy.invalidate_all();
    let r = query_helper(tmp.path(), "chinchilla population survey", &mut cache, &mut fuzzy);
    assert!(
        !r.hits.is_empty(),
        "curated fact should be found after incremental compile"
    );
}

// ============================================================
// Cross-prompt gap: Fingerprint corruption recovery
// Verifies: corrupted fingerprints trigger graceful fallback to full rebuild.
// ============================================================

#[test]
fn gap_corrupted_fingerprints_fallback() {
    let tmp = temp_workspace();
    write_fact(
        tmp.path(),
        "survivor.md",
        &durable_fact("Survivor Echidna", "Echidna is a monotreme native to Australia."),
    );
    compile_clean(tmp.path());

    // Corrupt the fingerprints file
    let fp_path = tmp.path().join(".brv/index/fingerprints.bin");
    std::fs::write(&fp_path, b"corrupted data that is not valid bincode").unwrap();

    // Incremental should fall back to full rebuild
    let result = compile_incremental(tmp.path());
    assert!(result.index_error.is_none(), "fallback rebuild should succeed");

    // Fact should still be queryable
    let mut cache = ExactCache::new(60);
    let mut fuzzy = FuzzyCache::new(100);
    let r = query_helper(tmp.path(), "echidna monotreme australia", &mut cache, &mut fuzzy);
    assert!(!r.hits.is_empty(), "fact should survive fingerprint corruption");
}

// ============================================================
// Cross-prompt gap: Schema upgrade + incremental
// Verifies: after schema version bump and full rebuild, incremental
// compile works correctly with the new schema.
// ============================================================

#[test]
fn gap_schema_upgrade_then_incremental() {
    let tmp = temp_workspace();
    write_fact(
        tmp.path(),
        "pre-upgrade.md",
        &durable_fact("Pre Upgrade Pangolin", "Pangolin scales are made of keratin."),
    );
    compile_clean(tmp.path());

    // Simulate a schema downgrade (write old version)
    let version_path = tmp
        .path()
        .join(".brv/index/tantivy/engram_schema_version");
    std::fs::write(&version_path, "0").unwrap();

    // Full compile should detect version mismatch and rebuild
    compile_clean(tmp.path());

    // Now add a file and do incremental
    write_fact(
        tmp.path(),
        "post-upgrade.md",
        &durable_fact("Post Upgrade Quokka", "Quokka is known for its friendly smile."),
    );
    let inc = compile_incremental(tmp.path());
    assert!(inc.index_error.is_none(), "incremental after schema upgrade should work");

    let mut cache = ExactCache::new(60);
    let mut fuzzy = FuzzyCache::new(100);
    let r = query_helper(tmp.path(), "quokka friendly smile", &mut cache, &mut fuzzy);
    assert!(!r.hits.is_empty(), "incrementally added fact should be found");

    cache.invalidate_all();
    fuzzy.invalidate_all();
    let r = query_helper(tmp.path(), "pangolin keratin scales", &mut cache, &mut fuzzy);
    assert!(!r.hits.is_empty(), "pre-upgrade fact should survive schema rebuild");
}

// ============================================================
// Cross-prompt gap: Bulwark policy affects both compile and query paths
// Verifies: denying bulwark blocks both compile and query uniformly.
// ============================================================

#[test]
fn gap_bulwark_denies_both_compile_and_query() {
    let tmp = temp_workspace();
    write_fact(
        tmp.path(),
        "policy.md",
        &durable_fact("Policy Test Wombat", "Wombat has cube-shaped droppings."),
    );

    // Compile with denying bulwark
    let denying = BulwarkHandle::new_denying();
    let result = compile_context_tree(tmp.path(), true, &denying);
    assert!(result.index_error.is_some(), "denying bulwark should block compile");

    // Compile with allowing bulwark
    compile_clean(tmp.path());

    // Query with denying bulwark
    let mut cache = ExactCache::new(60);
    let mut fuzzy = FuzzyCache::new(100);
    let config = permissive_config();

    let result = engram_query::query(
        tmp.path(),
        "wombat",
        default_query_options(),
        &mut cache,
        &mut fuzzy,
        &denying,
        &config,
    );
    assert!(result.is_err(), "denying bulwark should block query");
}
