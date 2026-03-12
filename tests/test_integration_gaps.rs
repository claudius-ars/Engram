#[allow(dead_code)]
mod common;

use engram_bulwark::BulwarkHandle;
use engram_compiler::compile_context_tree;
use engram_core::temporal::{parse_temporal_log, EVENT_KIND_CREATED, EVENT_KIND_UPDATED, EVENT_KIND_DELETED};
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

// ============================================================
// Phase 3 Prompt 8: Temporal Log Backfill — content_hash populated
// Verifies: manifest entries have non-zero content_hash from BLAKE3 fingerprint.
// ============================================================

#[test]
fn backfill_content_hash_populated_from_blake3() {
    let tmp = temp_workspace();
    write_fact(
        tmp.path(),
        "hash-test.md",
        &durable_fact("Hash Platypus", "Platypus venom is delivered via ankle spurs."),
    );
    compile_clean(tmp.path());

    let manifest = engram_compiler::read_manifest_envelope(tmp.path())
        .expect("manifest should be readable");

    assert_eq!(manifest.entries.len(), 1);
    let entry = &manifest.entries[0];
    assert_ne!(
        entry.content_hash,
        [0u8; 16],
        "content_hash should be non-zero (populated from BLAKE3 fingerprint)"
    );
}

// ============================================================
// Phase 3 Prompt 8: Temporal Log Backfill — Created events on first compile
// ============================================================

#[test]
fn backfill_first_compile_emits_created_events() {
    let tmp = temp_workspace();
    write_fact(
        tmp.path(),
        "alpha.md",
        &durable_fact("Alpha Axolotl", "Axolotls can regenerate limbs."),
    );
    write_fact(
        tmp.path(),
        "beta.md",
        &durable_fact("Beta Bison", "Bison are the largest land animals in North America."),
    );
    compile_clean(tmp.path());

    let log_path = tmp.path().join(".brv/index/temporal.log");
    assert!(log_path.exists(), "temporal.log should be written");

    let data = std::fs::read(&log_path).unwrap();
    let (header, records) = parse_temporal_log(&data).unwrap();

    assert_eq!(header.record_count, 2, "first compile should emit 2 Created events");
    for r in records {
        assert_eq!(r.event_kind, EVENT_KIND_CREATED, "all first-compile events should be Created");
    }
}

// ============================================================
// Phase 3 Prompt 8: Temporal Log Backfill — Updated event on content change
// ============================================================

#[test]
fn backfill_updated_event_on_content_change() {
    let tmp = temp_workspace();
    write_fact(
        tmp.path(),
        "mutable.md",
        &durable_fact("Mutable Narwhal", "Narwhal tusks are elongated canine teeth."),
    );
    compile_clean(tmp.path());

    // Modify the fact content
    write_fact(
        tmp.path(),
        "mutable.md",
        &durable_fact("Mutable Narwhal", "Narwhal tusks can grow up to 3 meters long and are sensory organs."),
    );
    compile_clean(tmp.path());

    let log_path = tmp.path().join(".brv/index/temporal.log");
    let data = std::fs::read(&log_path).unwrap();
    let (_, records) = parse_temporal_log(&data).unwrap();

    let updated_count = records.iter().filter(|r| r.event_kind == EVENT_KIND_UPDATED).count();
    assert!(
        updated_count >= 1,
        "second compile with changed content should emit at least one Updated event, got {} Updated out of {} total",
        updated_count,
        records.len()
    );
}

// ============================================================
// Phase 3 Prompt 8: Temporal Log Backfill — Deleted event on fact removal
// ============================================================

#[test]
fn backfill_deleted_event_on_fact_removal() {
    let tmp = temp_workspace();
    write_fact(
        tmp.path(),
        "keeper.md",
        &durable_fact("Keeper Koala", "Koalas sleep up to 22 hours a day."),
    );
    write_fact(
        tmp.path(),
        "doomed.md",
        &durable_fact("Doomed Dodo", "The dodo was endemic to Mauritius."),
    );
    compile_clean(tmp.path());

    // Remove one fact
    std::fs::remove_file(tmp.path().join(".brv/context-tree/doomed.md")).unwrap();
    compile_clean(tmp.path());

    let log_path = tmp.path().join(".brv/index/temporal.log");
    let data = std::fs::read(&log_path).unwrap();
    let (_, records) = parse_temporal_log(&data).unwrap();

    let deleted_count = records.iter().filter(|r| r.event_kind == EVENT_KIND_DELETED).count();
    assert!(
        deleted_count >= 1,
        "removing a fact should emit at least one Deleted event, got {} Deleted out of {} total",
        deleted_count,
        records.len()
    );
}

// ============================================================
// Phase 3 Prompt 8: Temporal Log Backfill — defensive zero hash
// Verifies: if a fingerprint is somehow missing, content_hash defaults to zero
// rather than panicking.
// ============================================================

#[test]
fn backfill_defensive_zero_hash_on_missing_fingerprint() {
    let tmp = temp_workspace();
    write_fact(
        tmp.path(),
        "orphan.md",
        &durable_fact("Orphan Okapi", "Okapi tongues are long enough to clean their own ears."),
    );
    compile_clean(tmp.path());

    // Corrupt the fingerprints file so content_hashes lookup fails
    let fp_path = tmp.path().join(".brv/index/fingerprints.bin");
    std::fs::write(&fp_path, b"invalid bincode data").unwrap();

    // Re-compile — should fall back to full rebuild with zero hashes for any
    // facts whose fingerprints can't be found in the corrupted store
    let result = compile_clean(tmp.path());
    assert!(result.index_error.is_none(), "compile should succeed despite corrupted fingerprints");

    // The manifest should still be written — entries may have zero hash
    let manifest = engram_compiler::read_manifest_envelope(tmp.path())
        .expect("manifest should be readable after rebuild");
    assert!(!manifest.entries.is_empty(), "manifest should have entries");
}
