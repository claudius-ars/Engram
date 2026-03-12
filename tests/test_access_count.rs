#[allow(dead_code)]
mod common;

use engram_bulwark::BulwarkHandle;
use engram_core::WorkspaceConfig;
use engram_query::{ExactCache, FuzzyCache, QueryOptions};

use common::{compile_clean, durable_fact, temp_workspace, write_fact};

fn permissive_config() -> WorkspaceConfig {
    WorkspaceConfig {
        score_threshold: 0.0,
        score_gap: 0.0,
        ..WorkspaceConfig::default()
    }
}

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
// Full round-trip: query → access log → compile → access_count
// ============================================================

#[test]
fn access_count_round_trip() {
    let tmp = temp_workspace();

    write_fact(
        tmp.path(),
        "capybara.md",
        &durable_fact("Capybara Facts", "The capybara is the largest living rodent."),
    );
    compile_clean(tmp.path());

    // Query 1: access_count should be 0
    let mut cache = ExactCache::new(60);
    let mut fuzzy = FuzzyCache::new(100);
    let r = query_helper(tmp.path(), "capybara rodent", &mut cache, &mut fuzzy);
    assert!(!r.hits.is_empty(), "should find capybara fact");
    assert_eq!(r.hits[0].access_count, 0, "access_count should start at 0");

    let original_importance = r.hits[0].importance;

    // Verify access log was written
    let log_path = tmp.path().join(".brv/index/access.log");
    assert!(log_path.exists(), "access.log should be created after query");
    let log_content = std::fs::read_to_string(&log_path).unwrap();
    assert!(
        !log_content.trim().is_empty(),
        "access.log should have entries"
    );
    assert!(
        log_content.contains("capybara"),
        "access.log should contain fact id"
    );

    // Recompile — should apply access counts and truncate log
    compile_clean(tmp.path());

    // Verify log is truncated
    let log_after = std::fs::read_to_string(&log_path).unwrap();
    assert!(
        log_after.trim().is_empty(),
        "access.log should be truncated after compile"
    );

    // Query 2: access_count should be 1, importance should have increased
    cache.invalidate_all();
    fuzzy.invalidate_all();
    let r = query_helper(tmp.path(), "capybara rodent", &mut cache, &mut fuzzy);
    assert!(!r.hits.is_empty(), "should find capybara fact");
    assert_eq!(
        r.hits[0].access_count, 1,
        "access_count should be 1 after one query + compile"
    );
    assert!(
        r.hits[0].importance > original_importance,
        "importance should have increased: {} > {}",
        r.hits[0].importance,
        original_importance
    );

    // The delta should be 0.001 (default importance_delta)
    let expected_importance = original_importance + 0.001;
    assert!(
        (r.hits[0].importance - expected_importance).abs() < 1e-10,
        "importance should increase by exactly importance_delta: got {}, expected {}",
        r.hits[0].importance,
        expected_importance
    );
}

// ============================================================
// Access log non-fatal: query succeeds even if log can't be written
// ============================================================

#[test]
fn access_log_nonfatal_on_query() {
    let tmp = temp_workspace();

    write_fact(
        tmp.path(),
        "pangolin.md",
        &durable_fact("Pangolin Info", "Pangolins are scaly anteaters."),
    );
    compile_clean(tmp.path());

    // Make the access log path a directory so write fails
    let log_path = tmp.path().join(".brv/index/access.log");
    std::fs::create_dir_all(&log_path).unwrap();

    // Query should still succeed despite access log failure
    let mut cache = ExactCache::new(60);
    let mut fuzzy = FuzzyCache::new(100);
    let r = query_helper(tmp.path(), "pangolin scaly", &mut cache, &mut fuzzy);
    assert!(!r.hits.is_empty(), "query should succeed even if access log fails");
}

// ============================================================
// Access tracking disabled: no log written
// ============================================================

#[test]
fn access_tracking_disabled_no_log() {
    let tmp = temp_workspace();

    write_fact(
        tmp.path(),
        "okapi.md",
        &durable_fact("Okapi Facts", "The okapi is related to the giraffe."),
    );

    // Write config with access_tracking disabled
    std::fs::write(
        tmp.path().join(".brv/engram.toml"),
        "[query]\nscore_threshold = 0.0\nscore_gap = 0.0\n\n[access_tracking]\nenabled = false\n",
    )
    .unwrap();

    compile_clean(tmp.path());

    // Query with access tracking disabled
    let mut cache = ExactCache::new(60);
    let mut fuzzy = FuzzyCache::new(100);
    let bulwark = BulwarkHandle::new_stub();
    let config = WorkspaceConfig {
        score_threshold: 0.0,
        score_gap: 0.0,
        access_tracking: engram_core::AccessTrackingConfig {
            enabled: false,
            ..Default::default()
        },
        ..WorkspaceConfig::default()
    };

    let r = engram_query::query(
        tmp.path(),
        "okapi giraffe",
        default_query_options(),
        &mut cache,
        &mut fuzzy,
        &bulwark,
        &config,
    )
    .unwrap();

    assert!(!r.hits.is_empty(), "should find okapi fact");

    // Access log should not exist
    let log_path = tmp.path().join(".brv/index/access.log");
    assert!(
        !log_path.exists(),
        "access.log should not be created when tracking is disabled"
    );
}

// ============================================================
// Multiple queries accumulate access count
// ============================================================

#[test]
fn multiple_queries_accumulate_access_count() {
    let tmp = temp_workspace();

    write_fact(
        tmp.path(),
        "echidna.md",
        &durable_fact("Echidna Facts", "Echidnas are spiny monotremes from Australia."),
    );
    compile_clean(tmp.path());

    // Query 3 times
    for _ in 0..3 {
        let mut cache = ExactCache::new(60);
        let mut fuzzy = FuzzyCache::new(100);
        let _ = query_helper(tmp.path(), "echidna monotreme", &mut cache, &mut fuzzy);
    }

    // Recompile
    compile_clean(tmp.path());

    // access_count should be 3
    let mut cache = ExactCache::new(60);
    let mut fuzzy = FuzzyCache::new(100);
    let r = query_helper(tmp.path(), "echidna monotreme", &mut cache, &mut fuzzy);
    assert!(!r.hits.is_empty());
    assert_eq!(
        r.hits[0].access_count, 3,
        "access_count should be 3 after three queries + compile"
    );
}

// ============================================================
// Stale generation entries are skipped during access count apply
// ============================================================

#[test]
fn access_count_stale_generation_skipped() {
    let tmp = temp_workspace();

    write_fact(
        tmp.path(),
        "wombat.md",
        &durable_fact("Wombat Facts", "Wombats produce cube-shaped droppings."),
    );

    // Gen 1: compile
    compile_clean(tmp.path());

    // Query → access log has entries with gen=1
    let mut cache = ExactCache::new(60);
    let mut fuzzy = FuzzyCache::new(100);
    let _ = query_helper(tmp.path(), "wombat droppings", &mut cache, &mut fuzzy);

    // Gen 2: compile → applies gen=1 entries, access_count becomes 1
    compile_clean(tmp.path());

    // Query → access log has entries with gen=2
    cache.invalidate_all();
    fuzzy.invalidate_all();
    let _ = query_helper(tmp.path(), "wombat droppings", &mut cache, &mut fuzzy);

    // Manually inject a stale entry (gen=1) into the access log before next compile.
    // The current log has gen=2 entries from the query above.
    let log_path = tmp.path().join(".brv/index/access.log");
    let existing = std::fs::read_to_string(&log_path).unwrap_or_default();
    let stale_line = r#"{"ts":1000,"fact_id":"wombat","agent":"engram","gen":1}"#;
    let injected = format!("{}{}\n", existing, stale_line);
    std::fs::write(&log_path, injected).unwrap();

    // Gen 3: compile → should apply gen=2 entries but skip gen=1 stale entry.
    // Note: access_count does not accumulate across compiles — each compile
    // re-parses markdown (access_count=0) and applies only the current log.
    // So we expect 1 (from the gen=2 entry), NOT 2 (which would require
    // the stale gen=1 injected entry to also be counted).
    compile_clean(tmp.path());

    cache.invalidate_all();
    fuzzy.invalidate_all();
    let r = query_helper(tmp.path(), "wombat droppings", &mut cache, &mut fuzzy);
    assert!(!r.hits.is_empty(), "should find wombat fact");
    // The gen=2 entry is counted (1), but the stale gen=1 entry is skipped.
    // Without stale filtering, this would be 2.
    assert_eq!(
        r.hits[0].access_count, 1,
        "stale gen=1 entry should be skipped; only gen=2 entry counted"
    );
}

// ============================================================
// Access count reflects only the latest compile cycle's log
// ============================================================

#[test]
fn access_count_reflects_latest_cycle() {
    let tmp = temp_workspace();

    write_fact(
        tmp.path(),
        "quoll.md",
        &durable_fact("Quoll Facts", "Quolls are carnivorous marsupials native to Australia."),
    );

    // Gen 1: compile
    compile_clean(tmp.path());

    // Query once, compile → access_count = 1
    let mut cache = ExactCache::new(60);
    let mut fuzzy = FuzzyCache::new(100);
    let _ = query_helper(tmp.path(), "quoll marsupial", &mut cache, &mut fuzzy);
    compile_clean(tmp.path());

    // Verify access_count = 1 (note: this query also writes to the access log!)
    cache.invalidate_all();
    fuzzy.invalidate_all();
    let r = query_helper(tmp.path(), "quoll marsupial", &mut cache, &mut fuzzy);
    assert!(!r.hits.is_empty());
    assert_eq!(r.hits[0].access_count, 1, "first cycle: 1 query → access_count=1");

    // Query twice more → log now has 3 entries (1 verification + 2 explicit)
    // Each compile re-parses markdown (access_count=0) and applies only the log.
    for _ in 0..2 {
        cache.invalidate_all();
        fuzzy.invalidate_all();
        let _ = query_helper(tmp.path(), "quoll marsupial", &mut cache, &mut fuzzy);
    }
    compile_clean(tmp.path());

    cache.invalidate_all();
    fuzzy.invalidate_all();
    let r = query_helper(tmp.path(), "quoll marsupial", &mut cache, &mut fuzzy);
    assert!(!r.hits.is_empty());
    assert_eq!(
        r.hits[0].access_count, 3,
        "second cycle: 3 queries in log (1 verification + 2 explicit) → access_count=3"
    );
}
