use std::path::Path;

use engram_bulwark::BulwarkHandle;
use engram_core::WorkspaceConfig;

use crate::causal_reader::CausalReader;
use crate::searcher::{freshness_bonus, BM25Searcher, SearchError};
use crate::QueryOptions;

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

/// Helper: compile custom facts from inline content.
fn compile_custom(facts: &[(&str, &str)]) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let context_tree = tmp.path().join(".brv").join("context-tree");
    std::fs::create_dir_all(&context_tree).unwrap();

    for (name, content) in facts {
        std::fs::write(context_tree.join(name), content).unwrap();
    }

    engram_compiler::compile_context_tree(tmp.path(), true, &BulwarkHandle::new_stub());
    tmp
}

fn default_config() -> WorkspaceConfig {
    // Use permissive defaults for existing tests so they don't get filtered
    WorkspaceConfig {
        score_threshold: 0.0,
        score_gap: 0.0,
        ..WorkspaceConfig::default()
    }
}

// --- Test 7: search on missing index returns IndexNotFound ---
#[test]
fn test_search_no_index() {
    let tmp = tempfile::tempdir().unwrap();
    let index_dir = tmp.path().join("nonexistent");
    let searcher = BM25Searcher::new(&index_dir);
    let result = searcher.search("test", &QueryOptions::default(), &default_config(), &CausalReader::empty(), None, None);
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), SearchError::IndexNotFound(_)));
}

// --- Test 8: search returns results ---
#[test]
fn test_search_returns_results() {
    let tmp = compile_fixtures(&["valid_legacy.md", "valid_engram.md"]);
    let index_dir = tmp.path().join(".brv").join("index").join("tantivy");
    let searcher = BM25Searcher::new(&index_dir);
    let results = searcher
        .search("Legacy", &QueryOptions::default(), &default_config(), &CausalReader::empty(), None, None)
        .unwrap();
    assert!(!results.is_empty());
    assert!(results[0]
        .hit
        .title
        .as_deref()
        .unwrap()
        .contains("Legacy"));
}

// --- Test 9: compound scoring ranks high-weight doc first ---
#[test]
fn test_compound_scoring() {
    let tmp = compile_custom(&[
        (
            "high.md",
            "---\ntitle: \"Unique Aardvark Fact\"\nimportance: 1.0\nconfidence: 1.0\nfactType: durable\n---\n\nUnique aardvark content here.\n",
        ),
        (
            "low.md",
            "---\ntitle: \"Unique Aardvark Low\"\nimportance: 0.3\nconfidence: 0.3\nfactType: durable\n---\n\nUnique aardvark content here too.\n",
        ),
    ]);

    let index_dir = tmp.path().join(".brv").join("index").join("tantivy");
    let searcher = BM25Searcher::new(&index_dir);
    let results = searcher
        .search("aardvark", &QueryOptions::default(), &default_config(), &CausalReader::empty(), None, None)
        .unwrap();

    assert!(results.len() >= 2);
    assert!(
        results[0].compound_score >= results[1].compound_score,
        "high-weight doc (score {:.3}) should rank above low-weight doc (score {:.3})",
        results[0].compound_score,
        results[1].compound_score,
    );
    assert!(results[0].hit.importance > results[1].hit.importance);
}

// --- Test 10: max_results limits output ---
#[test]
fn test_search_max_results() {
    let facts: Vec<_> = (0..5)
        .map(|i| {
            (
                format!("fact{}.md", i),
                format!(
                    "---\ntitle: \"Zebra Fact {}\"\nfactType: durable\n---\n\nZebra content number {}.\n",
                    i, i
                ),
            )
        })
        .collect();

    let fact_refs: Vec<(&str, &str)> = facts.iter().map(|(n, c)| (n.as_str(), c.as_str())).collect();
    let tmp = compile_custom(&fact_refs);

    let index_dir = tmp.path().join(".brv").join("index").join("tantivy");
    let searcher = BM25Searcher::new(&index_dir);
    let options = QueryOptions {
        max_results: 2,
        min_score: 0.0,
    };
    let results = searcher
        .search("zebra", &options, &default_config(), &CausalReader::empty(), None, None)
        .unwrap();
    assert!(results.len() <= 2);
}

// --- Test 11: empty query does not panic ---
#[test]
fn test_search_empty_query_fallback() {
    let tmp = compile_fixtures(&["valid_legacy.md"]);
    let index_dir = tmp.path().join(".brv").join("index").join("tantivy");
    let searcher = BM25Searcher::new(&index_dir);
    let result = searcher.search("", &QueryOptions::default(), &default_config(), &CausalReader::empty(), None, None);
    assert!(result.is_ok());
}

// === New Phase 2 Prompt 3 scoring tests ===

// --- Test: durable facts ignore recency ---
#[test]
fn test_durable_score_ignores_recency() {
    // Use identical titles and bodies so BM25 scores are equal
    let tmp = compile_custom(&[
        (
            "high_recency.md",
            "---\ntitle: \"Platypus Architecture\"\nfactType: durable\nimportance: 1.0\nconfidence: 1.0\nrecency: 1.0\ntags: [platypus]\n---\n\nPlatypus architecture details content.\n",
        ),
        (
            "low_recency.md",
            "---\ntitle: \"Platypus Architecture\"\nfactType: durable\nimportance: 1.0\nconfidence: 1.0\nrecency: 0.1\ntags: [platypus]\n---\n\nPlatypus architecture details content.\n",
        ),
    ]);

    let index_dir = tmp.path().join(".brv").join("index").join("tantivy");
    let searcher = BM25Searcher::new(&index_dir);
    let results = searcher
        .search("platypus architecture", &QueryOptions::default(), &default_config(), &CausalReader::empty(), None, None)
        .unwrap();

    assert!(results.len() >= 2, "should find both durable facts");

    // Both durable facts have identical content, importance, confidence.
    // Recency differs (1.0 vs 0.1) but durable scoring ignores recency.
    // So compound scores should be equal.
    let diff = (results[0].compound_score - results[1].compound_score).abs();
    assert!(
        diff < 0.001,
        "durable scores should be equal regardless of recency: {:.4} vs {:.4} (diff={:.4})",
        results[0].compound_score,
        results[1].compound_score,
        diff,
    );
}

// --- Test: expired state fact scores exactly 0.0 ---
#[test]
fn test_expired_state_scores_zero() {
    // valid_until set to 2020-01-01 — well in the past
    let tmp = compile_custom(&[(
        "expired.md",
        "---\ntitle: \"Expired Narwhal State\"\nfactType: state\nimportance: 1.0\nconfidence: 1.0\nvalidUntil: \"2020-01-01T00:00:00Z\"\n---\n\nNarwhal state that has expired.\n",
    )]);

    let index_dir = tmp.path().join(".brv").join("index").join("tantivy");
    let searcher = BM25Searcher::new(&index_dir);
    let results = searcher
        .search("narwhal", &QueryOptions::default(), &default_config(), &CausalReader::empty(), None, None)
        .unwrap();

    assert!(!results.is_empty(), "expired fact should still be returned by Tantivy");
    assert_eq!(
        results[0].compound_score, 0.0,
        "expired state fact should score exactly 0.0"
    );
}

// --- Test: non-expired state fact scores > 0.0 ---
#[test]
fn test_non_expired_state_scores_nonzero() {
    // No valid_until — never expires
    let tmp = compile_custom(&[(
        "current.md",
        "---\ntitle: \"Active Narwhal State\"\nfactType: state\nimportance: 1.0\nconfidence: 1.0\n---\n\nNarwhal state that is current.\n",
    )]);

    let index_dir = tmp.path().join(".brv").join("index").join("tantivy");
    let searcher = BM25Searcher::new(&index_dir);
    let results = searcher
        .search("narwhal", &QueryOptions::default(), &default_config(), &CausalReader::empty(), None, None)
        .unwrap();

    assert!(!results.is_empty());
    assert!(
        results[0].compound_score > 0.0,
        "non-expired state fact should have positive score, got: {}",
        results[0].compound_score
    );
}

// --- Test: fresh state scores higher than stale ---
#[test]
fn test_fresh_state_scores_higher_than_stale() {
    // One state updated "now", one updated 90 days ago
    let now = chrono::Utc::now();
    let ninety_days_ago = now - chrono::Duration::days(90);
    let now_str = now.format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let old_str = ninety_days_ago.format("%Y-%m-%dT%H:%M:%SZ").to_string();

    let fresh_content = format!(
        "---\ntitle: \"Fresh Walrus State\"\nfactType: state\nimportance: 1.0\nconfidence: 1.0\nupdatedAt: \"{}\"\n---\n\nWalrus state that is fresh.\n",
        now_str
    );
    let stale_content = format!(
        "---\ntitle: \"Stale Walrus State\"\nfactType: state\nimportance: 1.0\nconfidence: 1.0\nupdatedAt: \"{}\"\n---\n\nWalrus state that is stale.\n",
        old_str
    );

    let tmp = compile_custom(&[
        ("fresh.md", &fresh_content),
        ("stale.md", &stale_content),
    ]);

    let index_dir = tmp.path().join(".brv").join("index").join("tantivy");
    let searcher = BM25Searcher::new(&index_dir);
    let results = searcher
        .search("walrus state", &QueryOptions::default(), &default_config(), &CausalReader::empty(), None, None)
        .unwrap();

    assert!(results.len() >= 2, "should find both state facts");

    // Find the fresh and stale results
    let fresh = results.iter().find(|r| r.hit.title.as_deref() == Some("Fresh Walrus State"));
    let stale = results.iter().find(|r| r.hit.title.as_deref() == Some("Stale Walrus State"));

    assert!(fresh.is_some(), "should find fresh walrus");
    assert!(stale.is_some(), "should find stale walrus");

    assert!(
        fresh.unwrap().compound_score > stale.unwrap().compound_score,
        "fresh state ({:.4}) should score higher than stale state ({:.4})",
        fresh.unwrap().compound_score,
        stale.unwrap().compound_score
    );
}

// --- Test: event score uses recency ---
#[test]
fn test_event_score_uses_recency() {
    let tmp = compile_custom(&[
        (
            "recent_event.md",
            "---\ntitle: \"Recent Penguin Event\"\nfactType: event\nimportance: 1.0\nconfidence: 1.0\nrecency: 1.0\neventSequence: 1\n---\n\nPenguin event that is recent.\n",
        ),
        (
            "old_event.md",
            "---\ntitle: \"Old Penguin Event\"\nfactType: event\nimportance: 1.0\nconfidence: 1.0\nrecency: 0.2\neventSequence: 2\n---\n\nPenguin event that is old.\n",
        ),
    ]);

    let index_dir = tmp.path().join(".brv").join("index").join("tantivy");
    let searcher = BM25Searcher::new(&index_dir);
    let results = searcher
        .search("penguin event", &QueryOptions::default(), &default_config(), &CausalReader::empty(), None, None)
        .unwrap();

    assert!(results.len() >= 2, "should find both event facts");

    let recent = results.iter().find(|r| r.hit.title.as_deref() == Some("Recent Penguin Event"));
    let old = results.iter().find(|r| r.hit.title.as_deref() == Some("Old Penguin Event"));

    assert!(recent.is_some(), "should find recent event");
    assert!(old.is_some(), "should find old event");

    assert!(
        recent.unwrap().compound_score > old.unwrap().compound_score,
        "higher recency event ({:.4}) should score above lower recency event ({:.4})",
        recent.unwrap().compound_score,
        old.unwrap().compound_score
    );
}

// --- Test: score_threshold filters low scores ---
#[test]
fn test_score_threshold_filters_low_scores() {
    let tmp = compile_custom(&[(
        "low.md",
        "---\ntitle: \"Threshold Quokka Test\"\nfactType: durable\nimportance: 0.5\nconfidence: 0.5\n---\n\nQuokka content for threshold test.\n",
    )]);

    let index_dir = tmp.path().join(".brv").join("index").join("tantivy");
    let searcher = BM25Searcher::new(&index_dir);

    // With permissive threshold, the fact is returned
    let permissive = WorkspaceConfig {
        score_threshold: 0.0,
        score_gap: 0.0,
        ..WorkspaceConfig::default()
    };
    let results = searcher
        .search("quokka", &QueryOptions::default(), &permissive, &CausalReader::empty(), None, None)
        .unwrap();
    assert!(!results.is_empty(), "permissive threshold should return results");

    // Verify the actual score is below 0.99
    let actual_score = results[0].compound_score;
    assert!(
        actual_score < 0.99,
        "score {:.4} should be below 0.99 for importance=0.5, confidence=0.5",
        actual_score
    );

    // With strict threshold, the fact is filtered out
    let strict = WorkspaceConfig {
        score_threshold: 0.99,
        score_gap: 0.0,
        ..WorkspaceConfig::default()
    };
    let results = searcher
        .search("quokka", &QueryOptions::default(), &strict, &CausalReader::empty(), None, None)
        .unwrap();
    assert!(
        results.is_empty(),
        "strict threshold (0.99) should filter out fact with score {:.4}",
        actual_score
    );
}

// === Phase 2 Prompt 6: QueryHit expansion tests ===

// --- Test: keywords surfaced in QueryHit ---
#[test]
fn test_queryhit_keywords_surfaced() {
    let tmp = compile_custom(&[(
        "kw.md",
        "---\ntitle: \"Keyword Flamingo Test\"\nfactType: durable\nkeywords: [flamingo, migration, route]\n---\n\nFlamingo keyword test content.\n",
    )]);

    let index_dir = tmp.path().join(".brv").join("index").join("tantivy");
    let searcher = BM25Searcher::new(&index_dir);
    let results = searcher
        .search("flamingo", &QueryOptions::default(), &default_config(), &CausalReader::empty(), None, None)
        .unwrap();

    assert!(!results.is_empty(), "should find the flamingo fact");
    let hit = &results[0].hit;
    assert!(
        hit.keywords.contains(&"flamingo".to_string()),
        "keywords should contain 'flamingo', got: {:?}",
        hit.keywords
    );
    assert!(
        hit.keywords.contains(&"migration".to_string()),
        "keywords should contain 'migration', got: {:?}",
        hit.keywords
    );
}

// --- Test: related surfaced in QueryHit ---
#[test]
fn test_queryhit_related_surfaced() {
    let tmp = compile_custom(&[(
        "rel.md",
        "---\ntitle: \"Related Toucan Test\"\nfactType: durable\nrelated: [\"infra/k8s\", \"auth/sso\"]\n---\n\nToucan related test content.\n",
    )]);

    let index_dir = tmp.path().join(".brv").join("index").join("tantivy");
    let searcher = BM25Searcher::new(&index_dir);
    let results = searcher
        .search("toucan", &QueryOptions::default(), &default_config(), &CausalReader::empty(), None, None)
        .unwrap();

    assert!(!results.is_empty(), "should find the toucan fact");
    let hit = &results[0].hit;
    assert_eq!(hit.related, vec!["infra/k8s", "auth/sso"]);
}

// --- Test: maturity surfaced in QueryHit ---
#[test]
fn test_queryhit_maturity_surfaced() {
    let tmp = compile_custom(&[(
        "mat.md",
        "---\ntitle: \"Maturity Iguana Test\"\nfactType: durable\nmaturity: 0.6\n---\n\nIguana maturity test content.\n",
    )]);

    let index_dir = tmp.path().join(".brv").join("index").join("tantivy");
    let searcher = BM25Searcher::new(&index_dir);
    let results = searcher
        .search("iguana", &QueryOptions::default(), &default_config(), &CausalReader::empty(), None, None)
        .unwrap();

    assert!(!results.is_empty(), "should find the iguana fact");
    let hit = &results[0].hit;
    assert!(
        (hit.maturity - 0.6).abs() < 0.01,
        "maturity should be 0.6, got: {}",
        hit.maturity
    );
}

// --- Test: access_count and update_count default to zero ---
#[test]
fn test_queryhit_counts_default_zero() {
    let tmp = compile_custom(&[(
        "cnt.md",
        "---\ntitle: \"Count Chameleon Test\"\nfactType: durable\n---\n\nChameleon count test content.\n",
    )]);

    let index_dir = tmp.path().join(".brv").join("index").join("tantivy");
    let searcher = BM25Searcher::new(&index_dir);
    let results = searcher
        .search("chameleon", &QueryOptions::default(), &default_config(), &CausalReader::empty(), None, None)
        .unwrap();

    assert!(!results.is_empty(), "should find the chameleon fact");
    let hit = &results[0].hit;
    assert_eq!(hit.access_count, 0, "access_count should default to 0");
    assert_eq!(hit.update_count, 0, "update_count should default to 0");
}

// --- Test: freshness_bonus decay ---
#[test]
fn test_freshness_bonus_decay() {
    let now = chrono::Utc::now().timestamp();

    // 0 days ago: should be ≈ 1.5
    let bonus_0 = freshness_bonus(now, now);
    assert!(
        (bonus_0 - 1.5).abs() < 0.01,
        "0 days: expected ≈1.5, got {:.4}",
        bonus_0
    );

    // 30 days ago: should be ≈ 1.184
    let thirty_days = now - 30 * 86_400;
    let bonus_30 = freshness_bonus(thirty_days, now);
    assert!(
        (bonus_30 - 1.184).abs() < 0.01,
        "30 days: expected ≈1.184, got {:.4}",
        bonus_30
    );

    // Very old (365 days): should approach 1.0
    let year_ago = now - 365 * 86_400;
    let bonus_365 = freshness_bonus(year_ago, now);
    assert!(
        bonus_365 < 1.01,
        "365 days: expected ≈1.0, got {:.4}",
        bonus_365
    );

    // NULL_TIMESTAMP: should return exactly 1.0
    let bonus_null = freshness_bonus(i64::MIN, now);
    assert_eq!(bonus_null, 1.0, "NULL_TIMESTAMP should return 1.0");
}
