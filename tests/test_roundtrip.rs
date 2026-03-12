#[allow(unused)]
mod common;

use engram_bulwark::BulwarkHandle;
use engram_compiler::{curate, CurateOptions};
use engram_core::WorkspaceConfig;
use engram_query::{ExactCache, FuzzyCache, QueryOptions};

use common::{compile_clean, temp_workspace, write_fact, durable_fact};

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
    let config = WorkspaceConfig {
        score_threshold: 0.0,
        score_gap: 0.0,
        ..WorkspaceConfig::default()
    };
    engram_query::query(root, query_str, default_query_options(), cache, fuzzy_cache, &bulwark, &config)
        .expect("query should succeed")
}

// --- Test 19: curate sync immediately queryable ---
#[test]
fn test_curate_sync_immediately_queryable() {
    let tmp = temp_workspace();
    // Bootstrap an initial index so generation starts at 1
    write_fact(tmp.path(), "seed.md", &durable_fact("Seed Fact", "Initial seed content."));
    let r0 = compile_clean(tmp.path());
    let gen0 = r0.state.as_ref().unwrap().generation;

    let bulwark = BulwarkHandle::new_stub();
    let result = curate(
        tmp.path(),
        CurateOptions {
            summary: "Kubernetes cluster architecture note".to_string(),
            sync: true,
        },
        &bulwark,
    )
    .expect("curate --sync should succeed");

    // The curated file should exist
    assert!(result.written_path.exists(), "curated file should exist on disk");

    // Query the curated content
    let mut cache = ExactCache::new(60);
    let mut fuzzy = FuzzyCache::new(100);
    let r = query_helper(tmp.path(), "kubernetes cluster", &mut cache, &mut fuzzy);

    assert!(!r.hits.is_empty(), "curated fact should be immediately queryable");
    // Source path should point to the curated file
    assert!(
        r.hits[0].source_path.contains("curated"),
        "source_path should contain 'curated', got: {}",
        r.hits[0].source_path
    );
    // Generation should have incremented
    assert_eq!(
        r.meta.index_generation,
        gen0 + 1,
        "generation should increment after curate --sync"
    );
}

// --- Test 20: curate creates valid frontmatter ---
#[test]
fn test_curate_creates_valid_frontmatter() {
    let tmp = temp_workspace();
    let bulwark = BulwarkHandle::new_stub();

    let result = curate(
        tmp.path(),
        CurateOptions {
            summary: "Deployment pipeline for staging".to_string(),
            sync: true,
        },
        &bulwark,
    )
    .expect("curate --sync should succeed");

    // Read the written file
    let content = std::fs::read_to_string(&result.written_path).unwrap();

    // Parse frontmatter — extract between first two "---" lines
    let parts: Vec<&str> = content.splitn(3, "---").collect();
    assert!(parts.len() >= 3, "file should have YAML frontmatter delimiters");

    let frontmatter = parts[1].trim();
    let yaml: serde_json::Value = serde_yaml::from_str(frontmatter)
        .expect("frontmatter should be valid YAML");

    // Verify required fields
    assert_eq!(
        yaml["factType"].as_str().unwrap(),
        "durable",
        "curated facts should be durable type"
    );
    assert_eq!(
        yaml["confidence"].as_f64().unwrap(),
        1.0,
        "curated facts should have confidence 1.0"
    );
    assert!(
        !yaml["title"].as_str().unwrap().is_empty(),
        "title should be non-empty"
    );

    // Check that the filename contains an ISO 8601 date
    let filename = result.written_path.file_name().unwrap().to_str().unwrap();
    // Date format: YYYY-MM-DD
    let date_prefix = &filename[..10];
    assert!(
        date_prefix.chars().filter(|c| *c == '-').count() == 2,
        "filename should start with ISO 8601 date, got: {}",
        filename
    );
    // Verify the year is reasonable (2020+)
    let year: u32 = date_prefix[..4].parse().unwrap();
    assert!(year >= 2020, "year should be >= 2020, got: {}", year);
}

// --- Test 21: curate multiple facts all queryable ---
#[test]
fn test_curate_multiple_facts_all_queryable() {
    let tmp = temp_workspace();
    let bulwark = BulwarkHandle::new_stub();

    let summaries = [
        "Redis cache eviction policy",
        "PostgreSQL connection pooling setup",
        "Nginx reverse proxy configuration",
    ];

    for summary in &summaries {
        curate(
            tmp.path(),
            CurateOptions {
                summary: summary.to_string(),
                sync: true,
            },
            &bulwark,
        )
        .expect("curate --sync should succeed");
    }

    let mut cache = ExactCache::new(60);
    let mut fuzzy = FuzzyCache::new(100);

    // Query each distinct term
    let queries = ["Redis cache eviction", "PostgreSQL connection pooling", "Nginx reverse proxy"];

    for (i, query_str) in queries.iter().enumerate() {
        cache.invalidate_all();
        fuzzy.invalidate_all();

        let r = query_helper(tmp.path(), query_str, &mut cache, &mut fuzzy);
        assert!(
            !r.hits.is_empty(),
            "curated fact #{} ('{}') should be queryable",
            i + 1,
            query_str
        );
    }
}

// --- Test 22: curate async eventually consistent ---
#[test]
fn test_curate_async_eventually_consistent() {
    let tmp = temp_workspace();
    let bulwark = BulwarkHandle::new_stub();

    let result = curate(
        tmp.path(),
        CurateOptions {
            summary: "Async curate consistency test".to_string(),
            sync: false,
        },
        &bulwark,
    )
    .expect("curate (async) should succeed");

    // The .md file must exist on disk immediately
    assert!(
        result.written_path.exists(),
        "curated file should exist immediately after async curate"
    );

    // sync_compile_result should be None for async
    assert!(
        result.sync_compile_result.is_none(),
        "async curate should not have sync compile result"
    );

    // Now compile manually to make it queryable
    compile_clean(tmp.path());

    let mut cache = ExactCache::new(60);
    let mut fuzzy = FuzzyCache::new(100);
    let r = query_helper(tmp.path(), "async curate consistency", &mut cache, &mut fuzzy);

    assert!(
        !r.hits.is_empty(),
        "curated fact should be queryable after manual compile"
    );
}

// --- Test 23: curate sync generation consistency ---
#[test]
fn test_curate_sync_generation_consistency() {
    let tmp = temp_workspace();
    let bulwark = BulwarkHandle::new_stub();

    // Bootstrap with a seed fact so we have an initial index
    write_fact(tmp.path(), "seed.md", &durable_fact("Seed", "Initial content."));
    let r0 = compile_clean(tmp.path());
    let initial_gen = r0.state.as_ref().unwrap().generation;

    let mut cache = ExactCache::new(60);
    let mut fuzzy = FuzzyCache::new(100);

    for i in 1..=5u64 {
        curate(
            tmp.path(),
            CurateOptions {
                summary: format!("Generation consistency test number {}", i),
                sync: true,
            },
            &bulwark,
        )
        .expect("curate --sync should succeed");

        cache.invalidate_all();
        fuzzy.invalidate_all();

        let r = query_helper(tmp.path(), "generation consistency", &mut cache, &mut fuzzy);
        assert_eq!(
            r.meta.index_generation,
            initial_gen + i,
            "after curate --sync #{}, generation should be {}",
            i,
            initial_gen + i
        );
    }
}
