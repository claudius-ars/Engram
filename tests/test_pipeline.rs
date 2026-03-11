mod common;

use engram_bulwark::BulwarkHandle;
use engram_compiler::{compile_context_tree, read_manifest, ManifestEnvelope, MANIFEST_VERSION};
use engram_openclaw::{EngramPlugin, EnrichOptions};
use engram_query::{ExactCache, FuzzyCache, QueryOptions};

use common::{compile_clean, durable_fact, event_fact, set_dirty, state_fact, temp_workspace, write_fact};

fn default_query_options() -> QueryOptions {
    QueryOptions {
        max_results: 10,
        min_score: 0.0,
    }
}

fn query_helper(
    root: &std::path::Path,
    query_str: &str,
    cache: &mut ExactCache,
    fuzzy_cache: &mut FuzzyCache,
) -> engram_query::QueryResult {
    let bulwark = BulwarkHandle::new_stub();
    engram_query::query(root, query_str, default_query_options(), cache, fuzzy_cache, &bulwark)
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
    write_fact(tmp.path(), "k8s-high.md", &format!(
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
    ));

    // Mid-scoring fact
    write_fact(tmp.path(), "k8s-mid.md", &format!(
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
    ));

    // Low-scoring fact
    write_fact(tmp.path(), "k8s-low.md", &format!(
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
    ));

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

    // First query: Tier 2 (BM25 direct)
    let r = engram_query::query(
        tmp.path(), "cache pipeline verification", default_query_options(),
        &mut cache, &mut fuzzy, &bulwark,
    ).unwrap();
    assert_eq!(r.meta.cache_tier, 2, "first query should be Tier 2");

    // Same query again: Tier 0 (exact cache hit)
    let r = engram_query::query(
        tmp.path(), "cache pipeline verification", default_query_options(),
        &mut cache, &mut fuzzy, &bulwark,
    ).unwrap();
    assert_eq!(r.meta.cache_tier, 0, "identical query should hit Tier 0");

    // Invalidate exact cache, keep fuzzy cache populated
    cache.invalidate_all();

    // Query with reordered tokens — should hit Tier 1 (fuzzy/Jaccard)
    let r = engram_query::query(
        tmp.path(), "verification pipeline cache", default_query_options(),
        &mut cache, &mut fuzzy, &bulwark,
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

    let result = engram_query::query(
        tmp.path(), "deny test", default_query_options(),
        &mut cache, &mut fuzzy, &bulwark,
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
    assert_eq!(version.trim(), "1", "schema version should be updated to 1");
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
    assert!(envelope.entries.len() > 0, "manifest should have entries");
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
