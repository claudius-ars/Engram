use engram_bulwark::BulwarkHandle;
use engram_core::temporal::fnv1a_64;

use crate::result::QueryHit;
use crate::searcher::BM25Searcher;

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

fn make_sparse_temporal_hit(source_path_hash: u64) -> QueryHit {
    QueryHit {
        id: String::new(),
        title: None,
        source_path: format!("<temporal:{:016x}>", source_path_hash),
        tags: vec![],
        domain_tags: vec![],
        score: 1.0, // temporal score
        bm25_score: 0.0,
        fact_type: "durable".to_string(),
        confidence: 0.0,
        importance: 0.0,
        recency: 0.0,
        caused_by: vec![],
        causes: vec![],
        keywords: vec![],
        related: vec![],
        maturity: 0.0,
        access_count: 0,
        update_count: 0,
    }
}

// --- Test: enrichment of a present fact ---
// Note: enrichment uses O(N) segment scan (Phase 3 limitation).
// See enrich_temporal_hit() for the Phase 4 fix path.
#[test]
fn test_enrich_present_fact() {
    let tmp = compile_custom(&[(
        "enrich_fact.md",
        "---\ntitle: \"Enrichment Pangolin Fact\"\nfactType: durable\nimportance: 0.8\nconfidence: 0.9\nkeywords: [pangolin, armor]\ntags: [wildlife]\n---\n\nPangolin enrichment test content.\n",
    )]);

    let index_dir = tmp.path().join(".brv").join("index").join("tantivy");
    let bm25 = BM25Searcher::new(&index_dir);
    let open = bm25.open().unwrap();

    // The compiler stores the absolute path as source_path in Tantivy
    let abs_path = tmp.path().join(".brv").join("context-tree").join("enrich_fact.md");
    let abs_path_str = abs_path.to_string_lossy();
    let hash = fnv1a_64(abs_path_str.as_bytes());
    let sparse = make_sparse_temporal_hit(hash);
    assert!(sparse.source_path.starts_with("<temporal:"));

    let enriched = open.enrich_temporal_hit(sparse);

    // Enriched hit has Tantivy data
    assert_eq!(
        enriched.title.as_deref(),
        Some("Enrichment Pangolin Fact"),
        "title should be populated from Tantivy"
    );
    assert_eq!(enriched.source_path, abs_path_str);
    assert_eq!(enriched.fact_type, "durable");
    assert!(
        enriched.confidence > 0.0,
        "confidence should be non-zero, got: {}",
        enriched.confidence
    );
    assert!(
        enriched.importance > 0.0,
        "importance should be non-zero, got: {}",
        enriched.importance
    );
    assert!(
        enriched.tags.contains(&"wildlife".to_string()),
        "tags should contain 'wildlife', got: {:?}",
        enriched.tags
    );
    assert!(
        enriched.keywords.contains(&"pangolin".to_string()),
        "keywords should contain 'pangolin', got: {:?}",
        enriched.keywords
    );
}

// --- Test: enrichment of a deleted/missing fact ---
#[test]
fn test_enrich_deleted_fact() {
    let tmp = compile_custom(&[(
        "existing.md",
        "---\ntitle: \"Existing Fact\"\nfactType: durable\n---\n\nExisting content.\n",
    )]);

    let index_dir = tmp.path().join(".brv").join("index").join("tantivy");
    let bm25 = BM25Searcher::new(&index_dir);
    let open = bm25.open().unwrap();

    // Use a hash that doesn't match any document
    let nonexistent_hash = fnv1a_64(b"deleted_fact.md");
    let sparse = make_sparse_temporal_hit(nonexistent_hash);
    let original_source_path = sparse.source_path.clone();

    let result = open.enrich_temporal_hit(sparse);

    // Should return the sparse hit unchanged
    assert_eq!(
        result.source_path, original_source_path,
        "deleted fact should keep original <temporal:hash> source_path"
    );
    assert!(result.title.is_none(), "deleted fact should have no title");
    assert_eq!(result.score, 1.0, "temporal score should be preserved");
}

// --- Test: malformed source_path ---
#[test]
fn test_enrich_malformed_source_path() {
    let tmp = compile_custom(&[(
        "any.md",
        "---\ntitle: \"Any Fact\"\nfactType: durable\n---\n\nContent.\n",
    )]);

    let index_dir = tmp.path().join(".brv").join("index").join("tantivy");
    let bm25 = BM25Searcher::new(&index_dir);
    let open = bm25.open().unwrap();

    // Hit with a non-temporal source_path
    let mut hit = make_sparse_temporal_hit(0);
    hit.source_path = "not-a-temporal-format.md".to_string();

    let result = open.enrich_temporal_hit(hit);
    assert_eq!(
        result.source_path, "not-a-temporal-format.md",
        "malformed source_path should be returned unchanged"
    );

    // Hit with wrong-length hex
    let mut hit2 = make_sparse_temporal_hit(0);
    hit2.source_path = "<temporal:abcd>".to_string();

    let result2 = open.enrich_temporal_hit(hit2);
    assert_eq!(
        result2.source_path, "<temporal:abcd>",
        "short hex should be returned unchanged"
    );
}

// --- Test: score preservation ---
#[test]
fn test_enrich_preserves_temporal_score() {
    let tmp = compile_custom(&[(
        "scored.md",
        "---\ntitle: \"Scored Axolotl Fact\"\nfactType: durable\nimportance: 1.0\nconfidence: 1.0\n---\n\nAxolotl scored content.\n",
    )]);

    let index_dir = tmp.path().join(".brv").join("index").join("tantivy");
    let bm25 = BM25Searcher::new(&index_dir);
    let open = bm25.open().unwrap();

    let abs_path = tmp.path().join(".brv").join("context-tree").join("scored.md");
    let hash = fnv1a_64(abs_path.to_string_lossy().as_bytes());
    let mut sparse = make_sparse_temporal_hit(hash);
    sparse.score = 1.0; // temporal score
    sparse.bm25_score = 0.0;

    let enriched = open.enrich_temporal_hit(sparse);

    assert_eq!(
        enriched.score, 1.0,
        "enriched hit should keep temporal score, not BM25 score"
    );
    assert_eq!(
        enriched.bm25_score, 0.0,
        "bm25_score should remain 0.0"
    );
}

// --- Test: FAST field correctness (importance/confidence via column reader) ---
#[test]
fn test_enrich_fast_field_values() {
    let tmp = compile_custom(&[(
        "fast_fields.md",
        "---\ntitle: \"FAST Field Capybara\"\nfactType: event\nimportance: 0.75\nconfidence: 0.85\nrecency: 0.5\n---\n\nCapybara FAST field test.\n",
    )]);

    let index_dir = tmp.path().join(".brv").join("index").join("tantivy");
    let bm25 = BM25Searcher::new(&index_dir);
    let open = bm25.open().unwrap();

    let abs_path = tmp.path().join(".brv").join("context-tree").join("fast_fields.md");
    let hash = fnv1a_64(abs_path.to_string_lossy().as_bytes());
    let sparse = make_sparse_temporal_hit(hash);
    let enriched = open.enrich_temporal_hit(sparse);

    // FAST fields should match what was compiled
    assert!(
        (enriched.importance - 0.75).abs() < 0.01,
        "importance should be ~0.75, got: {}",
        enriched.importance
    );
    assert!(
        (enriched.confidence - 0.85).abs() < 0.01,
        "confidence should be ~0.85, got: {}",
        enriched.confidence
    );
    assert_eq!(
        enriched.fact_type, "event",
        "fact_type should be 'event'"
    );
}
