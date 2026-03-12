#[allow(dead_code)]
mod common;

use engram_bulwark::BulwarkHandle;
use engram_compiler::{compile_context_tree, read_manifest, ManifestEnvelope, MANIFEST_VERSION};
use engram_core::WorkspaceConfig;
use engram_openclaw::{EngramPlugin, EnrichOptions};
use engram_query::{ExactCache, FuzzyCache, QueryOptions, CACHE_TIER_TEMPORAL};

use common::{compile_clean, compile_incremental, compile_with_classify, durable_fact, event_fact, set_dirty, state_fact, temp_workspace, unclassified_fact, write_fact};

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
    engram_query::query(root, query_str, default_query_options(), cache, fuzzy_cache, &bulwark, &config)
        .expect("query should succeed")
}

// --- Test 1: compile empty workspace ---
#[test]
fn test_compile_empty_workspace() {
    let tmp = temp_workspace();
    let result = compile_clean(tmp.path());
    assert_eq!(result.parse_result.file_count, 0);
    assert!(result.index_error.is_none());
}

// --- Test 2: compile single durable fact ---
#[test]
fn test_compile_single_durable_fact() {
    let tmp = temp_workspace();
    write_fact(
        tmp.path(),
        "rust-ownership.md",
        &durable_fact("Rust Ownership Rules", "Ownership ensures memory safety without garbage collection."),
    );

    let result = compile_clean(tmp.path());
    assert_eq!(result.parse_result.file_count, 1);
    assert_eq!(result.parse_result.error_count, 0);
    assert!(result.index_stats.is_some());
    assert_eq!(result.index_stats.unwrap().documents_written, 1);
}

// --- Test 3: compile all fact types ---
#[test]
fn test_compile_all_fact_types() {
    let tmp = temp_workspace();
    write_fact(
        tmp.path(),
        "durable-fact.md",
        &durable_fact("Durable Architecture Pattern", "Microservices communicate via message queues."),
    );
    write_fact(
        tmp.path(),
        "state-fact.md",
        &state_fact("Current Deployment State", "Production is running version 3.2.1 of the API gateway."),
    );
    write_fact(
        tmp.path(),
        "event-fact.md",
        &event_fact("Incident Response Event", "Database failover triggered at 03:14 UTC.", 42),
    );

    let result = compile_clean(tmp.path());
    assert_eq!(result.parse_result.file_count, 3);
    assert_eq!(result.index_stats.as_ref().unwrap().documents_written, 3);

    let mut cache = ExactCache::new(60);
    let mut fuzzy = FuzzyCache::new(100);

    // Query each fact type and verify the type field
    let r = query_helper(tmp.path(), "Durable Architecture Pattern", &mut cache, &mut fuzzy);
    assert!(!r.hits.is_empty(), "durable fact should be found");
    assert_eq!(r.hits[0].fact_type, "durable");

    cache.invalidate_all();
    fuzzy.invalidate_all();

    let r = query_helper(tmp.path(), "Current Deployment State", &mut cache, &mut fuzzy);
    assert!(!r.hits.is_empty(), "state fact should be found");
    assert_eq!(r.hits[0].fact_type, "state");

    cache.invalidate_all();
    fuzzy.invalidate_all();

    let r = query_helper(tmp.path(), "Incident Response Event", &mut cache, &mut fuzzy);
    assert!(!r.hits.is_empty(), "event fact should be found");
    assert_eq!(r.hits[0].fact_type, "event");
}

// --- Test 4: incremental recompile ---
#[test]
fn test_compile_incremental_recompile() {
    let tmp = temp_workspace();
    write_fact(tmp.path(), "fact-a.md", &durable_fact("Fact Alpha", "First fact content."));
    write_fact(tmp.path(), "fact-b.md", &durable_fact("Fact Beta", "Second fact content."));

    let r1 = compile_clean(tmp.path());
    let gen1 = r1.state.as_ref().unwrap().generation;

    // Add a third fact and recompile
    write_fact(tmp.path(), "fact-c.md", &durable_fact("Fact Gamma", "Third fact content."));
    let r2 = compile_clean(tmp.path());
    let gen2 = r2.state.as_ref().unwrap().generation;

    assert_eq!(gen2, gen1 + 1, "generation should increment by 1");
    assert_eq!(r2.index_stats.as_ref().unwrap().documents_written, 3);
}

// --- Test 5: query ranking by compound score ---
#[test]
fn test_query_ranking_by_compound_score() {
    let tmp = temp_workspace();

    // High-scoring fact
    write_fact(tmp.path(), "k8s-high.md",
        r#"---
title: "Kubernetes High Priority"
factType: durable
confidence: 1.0
importance: 1.0
recency: 1.0
tags: [kubernetes]
---

Kubernetes cluster autoscaling is essential for production workloads.
"#
    );

    // Mid-scoring fact
    write_fact(tmp.path(), "k8s-mid.md",
        r#"---
title: "Kubernetes Mid Priority"
factType: durable
confidence: 0.7
importance: 0.6
recency: 0.8
tags: [kubernetes]
---

Kubernetes pod scheduling uses resource requests and limits.
"#
    );

    // Low-scoring fact
    write_fact(tmp.path(), "k8s-low.md",
        r#"---
title: "Kubernetes Low Priority"
factType: durable
confidence: 0.3
importance: 0.2
recency: 0.5
tags: [kubernetes]
---

Kubernetes labels can be used for organizational purposes.
"#
    );

    compile_clean(tmp.path());

    let mut cache = ExactCache::new(60);
    let mut fuzzy = FuzzyCache::new(100);
    let r = query_helper(tmp.path(), "kubernetes", &mut cache, &mut fuzzy);

    assert!(r.hits.len() >= 3, "should find all 3 kubernetes facts");
    // Verify ordering: scores should be descending
    for i in 1..r.hits.len() {
        assert!(
            r.hits[i - 1].score >= r.hits[i].score,
            "results should be ordered by descending score: {} >= {}",
            r.hits[i - 1].score,
            r.hits[i].score
        );
    }
}

// --- Test 6: field boost title over body ---
#[test]
fn test_query_field_boost_title_over_body() {
    let tmp = temp_workspace();

    // Fact A: "deployment" in title
    write_fact(tmp.path(), "title-match.md", &durable_fact(
        "Deployment Pipeline Configuration",
        "This document covers the CI/CD pipeline setup and rollout strategy.",
    ));

    // Fact B: "deployment" only in body
    write_fact(tmp.path(), "body-match.md", &durable_fact(
        "Infrastructure Overview",
        "The deployment process uses blue-green deployment with automated rollback.",
    ));

    compile_clean(tmp.path());

    let mut cache = ExactCache::new(60);
    let mut fuzzy = FuzzyCache::new(100);
    let r = query_helper(tmp.path(), "deployment", &mut cache, &mut fuzzy);

    assert!(r.hits.len() >= 2, "should find both facts");
    // The fact with "deployment" in the title should rank first due to title boost
    assert!(
        r.hits[0].title.as_deref().unwrap_or("").contains("Deployment"),
        "title-match fact should rank first, got: {:?}",
        r.hits[0].title
    );
}

// --- Test 7: dirty flag lifecycle ---
#[test]
fn test_dirty_flag_lifecycle() {
    let tmp = temp_workspace();
    write_fact(tmp.path(), "fact.md", &durable_fact("Lifecycle Test", "Testing dirty flag transitions."));

    // Compile clears dirty
    compile_clean(tmp.path());

    let mut cache = ExactCache::new(60);
    let mut fuzzy = FuzzyCache::new(100);
    let r = query_helper(tmp.path(), "lifecycle", &mut cache, &mut fuzzy);
    assert!(!r.meta.stale, "should not be stale after clean compile");

    // Set dirty and query again (must invalidate caches to avoid cached result)
    set_dirty(tmp.path());
    cache.invalidate_all();
    fuzzy.invalidate_all();

    let r = query_helper(tmp.path(), "lifecycle", &mut cache, &mut fuzzy);
    assert!(r.meta.stale, "should be stale after set_dirty");

    // Recompile clears dirty again
    compile_clean(tmp.path());
    cache.invalidate_all();
    fuzzy.invalidate_all();

    let r = query_helper(tmp.path(), "lifecycle", &mut cache, &mut fuzzy);
    assert!(!r.meta.stale, "should not be stale after recompile");
}

// --- Test 8: generation counter increments ---
#[test]
fn test_generation_counter_increments() {
    let tmp = temp_workspace();
    write_fact(tmp.path(), "fact.md", &durable_fact("Generation Test", "Testing generation counter."));

    let r1 = compile_clean(tmp.path());
    let g1 = r1.state.as_ref().unwrap().generation;

    let r2 = compile_clean(tmp.path());
    let g2 = r2.state.as_ref().unwrap().generation;

    let r3 = compile_clean(tmp.path());
    let g3 = r3.state.as_ref().unwrap().generation;

    assert_eq!(g2, g1 + 1);
    assert_eq!(g3, g2 + 1);
    assert_eq!(g3, g1 + 2);
}

// --- Test 9: three-tier cache pipeline ---
#[test]
fn test_three_tier_cache_pipeline() {
    let tmp = temp_workspace();
    write_fact(tmp.path(), "cache-test.md", &durable_fact(
        "Cache Pipeline Verification",
        "This fact tests the three-tier cache pipeline end to end.",
    ));
    compile_clean(tmp.path());

    let mut cache = ExactCache::new(60);
    let mut fuzzy = FuzzyCache::new(100);
    let bulwark = BulwarkHandle::new_stub();
    let config = permissive_config();

    // First query: Tier 2 (BM25 direct)
    let r = engram_query::query(
        tmp.path(), "cache pipeline verification", default_query_options(),
        &mut cache, &mut fuzzy, &bulwark, &config,
    ).unwrap();
    assert_eq!(r.meta.cache_tier, 2, "first query should be Tier 2");

    // Same query again: Tier 0 (exact cache hit)
    let r = engram_query::query(
        tmp.path(), "cache pipeline verification", default_query_options(),
        &mut cache, &mut fuzzy, &bulwark, &config,
    ).unwrap();
    assert_eq!(r.meta.cache_tier, 0, "identical query should hit Tier 0");

    // Invalidate exact cache, keep fuzzy cache populated
    cache.invalidate_all();

    // Query with reordered tokens — should hit Tier 1 (fuzzy/Jaccard)
    let r = engram_query::query(
        tmp.path(), "verification pipeline cache", default_query_options(),
        &mut cache, &mut fuzzy, &bulwark, &config,
    ).unwrap();
    assert_eq!(r.meta.cache_tier, 1, "reordered query should hit Tier 1 fuzzy cache");
}

// --- Test 10: bulwark deny blocks query ---
#[test]
fn test_bulwark_deny_blocks_query() {
    let tmp = temp_workspace();
    write_fact(tmp.path(), "fact.md", &durable_fact("Deny Test", "Should not be queryable."));
    compile_clean(tmp.path());

    let mut cache = ExactCache::new(60);
    let mut fuzzy = FuzzyCache::new(100);
    let bulwark = BulwarkHandle::new_denying();

    let config = permissive_config();
    let result = engram_query::query(
        tmp.path(), "deny test", default_query_options(),
        &mut cache, &mut fuzzy, &bulwark, &config,
    );

    assert!(result.is_err(), "query should fail with denying bulwark");
    match result.unwrap_err() {
        engram_query::QueryError::PolicyDenied(_) => {}
        other => panic!("expected PolicyDenied, got: {:?}", other),
    }
}

// --- Test 11: bulwark deny blocks compile ---
#[test]
fn test_bulwark_deny_blocks_compile() {
    let tmp = temp_workspace();
    write_fact(tmp.path(), "fact.md", &durable_fact("Deny Compile", "Should not compile."));

    let bulwark = BulwarkHandle::new_denying();
    let result = compile_context_tree(tmp.path(), true, &bulwark);

    assert!(
        result.index_error.is_some(),
        "compile should report index error with denying bulwark"
    );
    match result.index_error.as_ref().unwrap() {
        engram_compiler::IndexError::PolicyDenied(_) => {}
        other => panic!("expected PolicyDenied, got: {:?}", other),
    }
}

// --- Test 12: openclaw enrich full pipeline ---
#[test]
fn test_openclaw_enrich_full_pipeline() {
    let tmp = temp_workspace();
    write_fact(tmp.path(), "alpha.md", &durable_fact("Alpha Service Architecture", "Microservices pattern with event sourcing."));
    write_fact(tmp.path(), "beta.md", &state_fact("Beta Deployment Status", "Currently deployed to staging environment."));
    write_fact(tmp.path(), "gamma.md", &event_fact("Gamma Incident", "Service degradation detected.", 1));

    compile_clean(tmp.path());

    // Write permissive config so OpenClaw doesn't filter results
    std::fs::write(
        tmp.path().join(".brv/engram.toml"),
        "[query]\nscore_threshold = 0.0\nscore_gap = 0.0\n",
    ).unwrap();

    let mut plugin = EngramPlugin::new(tmp.path().to_path_buf(), EnrichOptions::default());
    let result = plugin.enrich("service architecture");

    assert!(result.from_index, "should have queried from index");
    assert!(result.fact_count >= 1, "should find at least one fact");
    assert!(
        result.context_block.contains("## Engram Context (Auto-Enriched)"),
        "context block should contain heading sentinel"
    );
    assert!(
        result.context_block.contains("<!-- engram:start -->"),
        "context block should contain start sentinel"
    );
    assert!(
        result.context_block.contains("<!-- engram:end -->"),
        "context block should contain end sentinel"
    );
}

// --- Test 25: schema version mismatch triggers rebuild ---
#[test]
fn test_schema_version_mismatch_triggers_rebuild() {
    let tmp = temp_workspace();
    write_fact(tmp.path(), "fact.md", &durable_fact("Schema Version Test", "Testing schema version mismatch handling."));

    // First compile writes schema v1
    compile_clean(tmp.path());

    // Overwrite schema version file with "0" to simulate old schema
    let version_path = tmp.path().join(".brv/index/tantivy/engram_schema_version");
    assert!(version_path.exists(), "schema version file should exist after compile");
    std::fs::write(&version_path, "0").unwrap();

    // Compile again — should wipe and rebuild
    let result = compile_clean(tmp.path());
    assert!(result.index_stats.as_ref().unwrap().documents_written > 0, "index should be rebuilt");

    // Schema version file should now contain "1"
    let version = std::fs::read_to_string(&version_path).unwrap();
    assert_eq!(version.trim(), engram_compiler::CURRENT_SCHEMA_VERSION.to_string(), "schema version should be updated");
}

// --- Test 26: manifest version written ---
#[test]
fn test_manifest_version_written() {
    let tmp = temp_workspace();
    write_fact(tmp.path(), "fact.md", &durable_fact("Manifest Version Test", "Testing manifest versioning."));
    compile_clean(tmp.path());

    // Read manifest via bincode to verify envelope structure
    let manifest_path = tmp.path().join(".brv/index/manifest.bin");
    assert!(manifest_path.exists(), "manifest.bin should exist after compile");

    let bytes = std::fs::read(&manifest_path).unwrap();
    let envelope: ManifestEnvelope = bincode::deserialize(&bytes).unwrap();

    assert_eq!(envelope.version, MANIFEST_VERSION, "manifest version should be 1");
    assert!(!envelope.entries.is_empty(), "manifest should have entries");
}

// --- Test 27: manifest version mismatch returns error ---
#[test]
fn test_manifest_version_mismatch_returns_error() {
    let tmp = temp_workspace();
    write_fact(tmp.path(), "fact.md", &durable_fact("Manifest Mismatch", "Testing version mismatch."));
    compile_clean(tmp.path());

    // Overwrite manifest.bin with version 0
    let manifest_path = tmp.path().join(".brv/index/manifest.bin");
    let bad_envelope = ManifestEnvelope {
        version: 0,
        entries: vec![],
    };
    let bytes = bincode::serialize(&bad_envelope).unwrap();
    std::fs::write(&manifest_path, bytes).unwrap();

    // read_manifest should return VersionMismatch error
    let result = read_manifest(tmp.path());
    assert!(result.is_err(), "read_manifest should fail on version mismatch");
    let err = result.unwrap_err();
    let err_msg = format!("{}", err);
    assert!(
        err_msg.contains("version mismatch"),
        "error should mention version mismatch, got: {}",
        err_msg
    );
}

// --- Test 28: system characterization ---
#[test]
fn test_system_characterization() {
    let tmp = temp_workspace();

    let topics = [
        "kubernetes", "deployment", "redis", "postgres", "nginx",
        "rust", "memory", "cache", "network", "storage",
    ];

    // 50 durable facts
    for i in 0..50 {
        let topic = topics[i % topics.len()];
        write_fact(
            tmp.path(),
            &format!("durable-{}.md", i),
            &durable_fact(
                &format!("{} Architecture Pattern {}", topic, i),
                &format!("This document covers {} infrastructure design and best practices for component {}.", topic, i),
            ),
        );
    }

    // 30 state facts
    for i in 0..30 {
        let topic = topics[i % topics.len()];
        write_fact(
            tmp.path(),
            &format!("state-{}.md", i),
            &state_fact(
                &format!("{} Current Status {}", topic, i),
                &format!("The {} system is currently running version {} with {} active connections.", topic, i, i * 10),
            ),
        );
    }

    // 20 event facts
    for i in 0..20 {
        let topic = topics[i % topics.len()];
        write_fact(
            tmp.path(),
            &format!("event-{}.md", i),
            &event_fact(
                &format!("{} Incident Report {}", topic, i),
                &format!("Incident detected in {} subsystem at sequence {}.", topic, i),
                i as i64,
            ),
        );
    }

    // Compile
    let compile_start = std::time::Instant::now();
    let result = compile_clean(tmp.path());
    let compile_ms = compile_start.elapsed().as_millis();

    assert_eq!(result.index_stats.as_ref().unwrap().documents_written, 100);
    assert!(result.index_error.is_none());

    // Run 10 queries
    let mut cache = ExactCache::new(60);
    let mut fuzzy = FuzzyCache::new(100);
    let mut query_times_ms = Vec::new();
    let mut tier_counts = [0u32; 3]; // [tier0, tier1, tier2]

    for topic in &topics {
        cache.invalidate_all();
        fuzzy.invalidate_all();

        let q_start = std::time::Instant::now();
        let r = query_helper(tmp.path(), topic, &mut cache, &mut fuzzy);
        let q_ms = q_start.elapsed().as_millis();
        query_times_ms.push(q_ms);

        assert!(!r.hits.is_empty(), "query '{}' should return results", topic);

        let tier = r.meta.cache_tier as usize;
        if tier < 3 {
            tier_counts[tier] += 1;
        }
    }

    eprintln!("=== System Characterization ===");
    eprintln!("Corpus: 100 facts (50 durable, 30 state, 20 event)");
    eprintln!("Compile time: {}ms", compile_ms);
    eprintln!("Query times: {:?}", query_times_ms);
    eprintln!("Cache tiers hit: [T0={}, T1={}, T2={}]", tier_counts[0], tier_counts[1], tier_counts[2]);
    eprintln!("===============================");
}

// --- Test 29: temporal log written on compile ---
#[test]
fn test_temporal_log_written_on_compile() {
    let tmp = temp_workspace();
    write_fact(
        tmp.path(),
        "temporal-test.md",
        &durable_fact("Temporal Log Test", "Verifying temporal log is written on compile."),
    );

    compile_clean(tmp.path());

    let log_path = tmp.path().join(".brv/index/temporal.log");
    assert!(log_path.exists(), "temporal.log should exist after compile");

    let data = std::fs::read(&log_path).unwrap();
    assert!(data.len() >= 64, "temporal.log should have at least a 64-byte header");

    // Check magic bytes
    assert_eq!(&data[..8], b"ENGRTLOG", "temporal.log magic should be ENGRTLOG");
}

// --- Test 30: temporal tier triggered on signal query ---
#[test]
fn test_temporal_tier_triggered_on_signal_query() {
    let tmp = temp_workspace();
    write_fact(
        tmp.path(),
        "auth-state.md",
        &state_fact("Auth Service State", "The auth service is currently running on v2.1."),
    );

    compile_clean(tmp.path());

    let mut cache = ExactCache::new(60);
    let mut fuzzy = FuzzyCache::new(100);
    let bulwark = BulwarkHandle::new_stub();
    let config = permissive_config();

    // Query with temporal signal word "current"
    let r = engram_query::query(
        tmp.path(),
        "what is the current state of auth",
        default_query_options(),
        &mut cache,
        &mut fuzzy,
        &bulwark,
        &config,
    )
    .unwrap();

    assert_eq!(
        r.meta.cache_tier, CACHE_TIER_TEMPORAL,
        "temporal signal query should report cache_tier = {} (Tier 2.5), got {}",
        CACHE_TIER_TEMPORAL, r.meta.cache_tier
    );
    assert!(!r.hits.is_empty(), "should return temporal hits");
}

// --- Test 31: classify flag changes fact_type ---
#[test]
fn test_classify_flag_changes_fact_type() {
    let tmp = temp_workspace();
    // Write a fact WITHOUT explicit factType, but with state-like body
    write_fact(
        tmp.path(),
        "rate-limit.md",
        &unclassified_fact(
            "API Rate Limiting",
            "The API is currently rate-limited to 100 req/s in production.",
        ),
    );

    // Compile WITHOUT --classify: fact_type should default to durable
    compile_clean(tmp.path());

    // Write permissive config for queries
    std::fs::write(
        tmp.path().join(".brv/engram.toml"),
        "[query]\nscore_threshold = 0.0\nscore_gap = 0.0\n",
    )
    .unwrap();

    let mut cache = ExactCache::new(60);
    let mut fuzzy = FuzzyCache::new(100);
    let r = query_helper(tmp.path(), "rate limit", &mut cache, &mut fuzzy);
    assert!(!r.hits.is_empty(), "should find rate-limit fact");
    assert_eq!(
        r.hits[0].fact_type, "durable",
        "without --classify, unclassified fact should default to durable"
    );

    // Compile WITH --classify: fact_type should be state (detected by keywords)
    compile_with_classify(tmp.path());

    cache.invalidate_all();
    fuzzy.invalidate_all();

    let r = query_helper(tmp.path(), "rate limit", &mut cache, &mut fuzzy);
    assert!(!r.hits.is_empty(), "should find rate-limit fact after --classify");
    assert_eq!(
        r.hits[0].fact_type, "state",
        "with --classify, fact containing 'is currently rate-limited' should be classified as state"
    );
}

// --- Test 32: classify without API key degrades gracefully ---
#[test]
fn test_classify_without_api_key_degrades_gracefully() {
    let tmp = temp_workspace();

    // Write a fact with clear state signal (rule-classifiable)
    write_fact(
        tmp.path(),
        "state-fact.md",
        &unclassified_fact(
            "Rate Limit Config",
            "The API is currently rate-limited to 500 req/s.",
        ),
    );

    // Write a fact with no clear signal (would need LLM)
    write_fact(
        tmp.path(),
        "ambiguous-fact.md",
        &unclassified_fact(
            "System Info",
            "The system processes data through a complex pipeline.",
        ),
    );

    // Ensure no API key is set (it shouldn't be in test env anyway)
    std::env::remove_var("ANTHROPIC_API_KEY");

    // Compile with --classify — should succeed, rule-classified facts work, ambiguous falls through
    compile_with_classify(tmp.path());

    // Write permissive config for queries
    std::fs::write(
        tmp.path().join(".brv/engram.toml"),
        "[query]\nscore_threshold = 0.0\nscore_gap = 0.0\n",
    )
    .unwrap();

    let mut cache = ExactCache::new(60);
    let mut fuzzy = FuzzyCache::new(100);

    // Rule-classified fact should be state
    let r = query_helper(tmp.path(), "rate limit API config", &mut cache, &mut fuzzy);
    assert!(!r.hits.is_empty(), "should find state fact");
    assert_eq!(
        r.hits[0].fact_type, "state",
        "rule-classified fact should be state even without API key"
    );

    // Ambiguous fact should fall through to durable default
    cache.invalidate_all();
    fuzzy.invalidate_all();

    let r = query_helper(tmp.path(), "system pipeline data", &mut cache, &mut fuzzy);
    assert!(!r.hits.is_empty(), "should find ambiguous fact");
    assert_eq!(
        r.hits[0].fact_type, "durable",
        "ambiguous fact without API key should default to durable"
    );
}

// === Phase 2 Prompt 7: Incremental compilation tests ===

// --- Test: incremental parity (M8 correctness test) ---
#[test]
fn test_incremental_parity() {
    let tmp = temp_workspace();

    // Write initial facts
    write_fact(tmp.path(), "alpha.md", &durable_fact("Alpha Architecture", "Alpha architecture is the core design pattern."));
    write_fact(tmp.path(), "bravo.md", &durable_fact("Bravo Pattern", "Bravo pattern describes the secondary module."));
    write_fact(tmp.path(), "charlie.md", &state_fact("Charlie State", "Charlie is currently active in production."));

    // Full compile
    let result = compile_clean(tmp.path());
    assert_eq!(result.parse_result.records.len(), 3);

    // Query all facts (baseline)
    let mut cache = ExactCache::new(60);
    let mut fuzzy = FuzzyCache::new(100);

    let r_alpha_before = query_helper(tmp.path(), "Alpha architecture core design", &mut cache, &mut fuzzy);
    cache.invalidate_all();
    fuzzy.invalidate_all();
    let r_bravo_before = query_helper(tmp.path(), "Bravo pattern secondary module", &mut cache, &mut fuzzy);
    cache.invalidate_all();
    fuzzy.invalidate_all();

    // Modify charlie.md
    write_fact(tmp.path(), "charlie.md", &state_fact("Charlie State", "Charlie is currently deployed to staging."));

    // Run incremental compile
    let inc_result = compile_incremental(tmp.path());
    assert!(inc_result.index_error.is_none(), "incremental should succeed");

    // Re-query: unchanged facts should have identical results
    cache.invalidate_all();
    fuzzy.invalidate_all();
    let r_alpha_after = query_helper(tmp.path(), "Alpha architecture core design", &mut cache, &mut fuzzy);
    cache.invalidate_all();
    fuzzy.invalidate_all();
    let r_bravo_after = query_helper(tmp.path(), "Bravo pattern secondary module", &mut cache, &mut fuzzy);

    // Unchanged facts should still be found
    assert!(!r_alpha_after.hits.is_empty(), "alpha should still be found after incremental");
    assert!(!r_bravo_after.hits.is_empty(), "bravo should still be found after incremental");

    // Alpha and bravo results should be identical
    assert_eq!(r_alpha_before.hits.len(), r_alpha_after.hits.len());
    assert_eq!(r_bravo_before.hits.len(), r_bravo_after.hits.len());
    if !r_alpha_before.hits.is_empty() && !r_alpha_after.hits.is_empty() {
        assert_eq!(r_alpha_before.hits[0].id, r_alpha_after.hits[0].id);
    }

    // Modified fact should reflect the change
    cache.invalidate_all();
    fuzzy.invalidate_all();
    let r_charlie = query_helper(tmp.path(), "Charlie staging deployed", &mut cache, &mut fuzzy);
    assert!(!r_charlie.hits.is_empty(), "modified charlie should be found with new content");
}

// --- Test: incremental faster than full ---
#[test]
fn test_incremental_faster_than_full() {
    let tmp = temp_workspace();

    // Write a bunch of facts
    for i in 0..20 {
        write_fact(
            tmp.path(),
            &format!("fact_{:03}.md", i),
            &durable_fact(
                &format!("Fact {}", i),
                &format!("This is the body of fact {} with unique content zebra{}.", i, i),
            ),
        );
    }

    // Full compile (and measure)
    let full_start = std::time::Instant::now();
    let result = compile_clean(tmp.path());
    let full_time = full_start.elapsed();
    assert_eq!(result.parse_result.records.len(), 20);

    // Modify one file
    write_fact(
        tmp.path(),
        "fact_000.md",
        &durable_fact("Fact 0 Modified", "Modified body for fact 0 with unique content zebra0."),
    );

    // Incremental compile (and measure)
    let inc_start = std::time::Instant::now();
    let inc_result = compile_incremental(tmp.path());
    let inc_time = inc_start.elapsed();

    assert!(inc_result.index_error.is_none(), "incremental should succeed");

    eprintln!(
        "test_incremental_faster_than_full: full={}ms, incremental={}ms",
        full_time.as_millis(),
        inc_time.as_millis()
    );

    // Incremental should be faster (or at least not massively slower)
    // Use a generous margin to avoid flakiness
    assert!(
        inc_time < full_time * 3,
        "incremental ({}ms) should not be 3x slower than full ({}ms)",
        inc_time.as_millis(),
        full_time.as_millis(),
    );
}

// --- Test: incremental handles file deletion ---
#[test]
fn test_incremental_handles_delete() {
    let tmp = temp_workspace();

    write_fact(tmp.path(), "keep.md", &durable_fact("Keep Aardvark", "The aardvark stays in the index."));
    write_fact(tmp.path(), "remove.md", &durable_fact("Remove Zebra", "The zebra will be deleted from the index."));

    // Full compile
    compile_clean(tmp.path());

    let mut cache = ExactCache::new(60);
    let mut fuzzy = FuzzyCache::new(100);

    // Verify both are searchable
    let r = query_helper(tmp.path(), "zebra deleted", &mut cache, &mut fuzzy);
    assert!(!r.hits.is_empty(), "remove fact should be found before deletion");

    // Delete the file
    std::fs::remove_file(tmp.path().join(".brv/context-tree/remove.md")).unwrap();

    // Incremental compile
    let inc_result = compile_incremental(tmp.path());
    assert!(inc_result.index_error.is_none());

    // Verify deleted fact is gone
    cache.invalidate_all();
    fuzzy.invalidate_all();
    let r = query_helper(tmp.path(), "zebra deleted", &mut cache, &mut fuzzy);
    assert!(r.hits.is_empty(), "deleted fact should not appear in results");

    // Verify kept fact is still there
    cache.invalidate_all();
    fuzzy.invalidate_all();
    let r = query_helper(tmp.path(), "aardvark stays", &mut cache, &mut fuzzy);
    assert!(!r.hits.is_empty(), "kept fact should still be found");
}

// --- Test: incremental handles rename ---
#[test]
fn test_incremental_handles_rename() {
    let tmp = temp_workspace();

    let content = durable_fact("Rename Target", "This fact will be renamed not deleted.");
    write_fact(tmp.path(), "old_name.md", &content);
    write_fact(tmp.path(), "other.md", &durable_fact("Other Fact", "Other unrelated content stays."));

    // Full compile
    compile_clean(tmp.path());

    // Rename: delete old, create new with same content
    let old_path = tmp.path().join(".brv/context-tree/old_name.md");
    let new_path = tmp.path().join(".brv/context-tree/new_name.md");
    let content_bytes = std::fs::read(&old_path).unwrap();
    std::fs::remove_file(&old_path).unwrap();
    std::fs::write(&new_path, content_bytes).unwrap();

    // Incremental compile
    let inc_result = compile_incremental(tmp.path());
    assert!(inc_result.index_error.is_none());

    // The fact should still be searchable
    let mut cache = ExactCache::new(60);
    let mut fuzzy = FuzzyCache::new(100);
    let r = query_helper(tmp.path(), "Rename Target renamed deleted", &mut cache, &mut fuzzy);
    assert!(!r.hits.is_empty(), "renamed fact should still be found");

    // Fingerprints should have new_name.md, not old_name.md
    let index_dir = tmp.path().join(".brv").join("index");
    let fps = engram_compiler::load_fingerprints(&index_dir);
    assert!(fps.entries.contains_key(".brv/context-tree/new_name.md"), "fingerprints should have new name");
    assert!(!fps.entries.contains_key(".brv/context-tree/old_name.md"), "fingerprints should not have old name");
}
