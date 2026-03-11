use std::path::Path;

use engram_bulwark::BulwarkHandle;

use crate::searcher::{BM25Searcher, SearchError};
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

// --- Test 7: search on missing index returns IndexNotFound ---
#[test]
fn test_search_no_index() {
    let tmp = tempfile::tempdir().unwrap();
    let index_dir = tmp.path().join("nonexistent");
    let searcher = BM25Searcher::new(&index_dir);
    let result = searcher.search("test", &QueryOptions::default());
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), SearchError::IndexNotFound(_)));
}

// --- Test 8: search returns results ---
#[test]
fn test_search_returns_results() {
    let tmp = compile_fixtures(&["valid_legacy.md", "valid_engram.md"]);
    let index_dir = tmp.path().join(".brv").join("index").join("tantivy");
    let searcher = BM25Searcher::new(&index_dir);
    let results = searcher.search("Legacy", &QueryOptions::default()).unwrap();
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
    let tmp = tempfile::tempdir().unwrap();
    let context_tree = tmp.path().join(".brv").join("context-tree");
    std::fs::create_dir_all(&context_tree).unwrap();

    // High-weight document
    std::fs::write(
        context_tree.join("high.md"),
        "---\ntitle: \"Unique Aardvark Fact\"\nimportance: 1.0\nconfidence: 1.0\nfactType: durable\n---\n\nUnique aardvark content here.\n",
    )
    .unwrap();

    // Low-weight document
    std::fs::write(
        context_tree.join("low.md"),
        "---\ntitle: \"Unique Aardvark Low\"\nimportance: 0.3\nconfidence: 0.3\nfactType: durable\n---\n\nUnique aardvark content here too.\n",
    )
    .unwrap();

    engram_compiler::compile_context_tree(tmp.path(), true, &BulwarkHandle::new_stub());

    let index_dir = tmp.path().join(".brv").join("index").join("tantivy");
    let searcher = BM25Searcher::new(&index_dir);
    let results = searcher.search("aardvark", &QueryOptions::default()).unwrap();

    assert!(results.len() >= 2);
    // High-weight document should rank first
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
    let tmp = tempfile::tempdir().unwrap();
    let context_tree = tmp.path().join(".brv").join("context-tree");
    std::fs::create_dir_all(&context_tree).unwrap();

    for i in 0..5 {
        std::fs::write(
            context_tree.join(format!("fact{}.md", i)),
            format!(
                "---\ntitle: \"Zebra Fact {}\"\nfactType: durable\n---\n\nZebra content number {}.\n",
                i, i
            ),
        )
        .unwrap();
    }

    engram_compiler::compile_context_tree(tmp.path(), true, &BulwarkHandle::new_stub());

    let index_dir = tmp.path().join(".brv").join("index").join("tantivy");
    let searcher = BM25Searcher::new(&index_dir);
    let options = QueryOptions {
        max_results: 2,
        min_score: 0.0,
    };
    let results = searcher.search("zebra", &options).unwrap();
    assert!(results.len() <= 2);
}

// --- Test 11: empty query does not panic ---
#[test]
fn test_search_empty_query_fallback() {
    let tmp = compile_fixtures(&["valid_legacy.md"]);
    let index_dir = tmp.path().join(".brv").join("index").join("tantivy");
    let searcher = BM25Searcher::new(&index_dir);
    let result = searcher.search("", &QueryOptions::default());
    assert!(result.is_ok());
}
