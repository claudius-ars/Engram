use std::path::Path;

use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::Index;

use engram_bulwark::BulwarkHandle;

use crate::compile_context_tree;
use crate::curator::{curate, CurateOptions};
use crate::indexer::build_schema;
use crate::state::read_state;

/// M4 correctness test: curate a fact with --sync, immediately query for it,
/// verify it appears in results. Zero failures across 20 sequential pairs.
#[test]
fn test_m4_sync_consistency() {
    let tmp = tempfile::tempdir().unwrap();
    let context_tree = tmp.path().join(".brv").join("context-tree");
    std::fs::create_dir_all(&context_tree).unwrap();

    // Copy a fixture for initial content
    let fixtures_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures");
    std::fs::copy(
        fixtures_dir.join("valid_legacy.md"),
        context_tree.join("valid_legacy.md"),
    )
    .unwrap();

    // Initial compile
    compile_context_tree(tmp.path(), true, &BulwarkHandle::new_stub());

    let schema = build_schema();

    for i in 0..20 {
        let unique_term = format!("engram-test-fact-{}", i);
        let summary = format!("{} the quick brown fox jumped {} times", unique_term, i);

        // Curate with --sync
        let options = CurateOptions {
            summary,
            sync: true,
        };
        let result = curate(tmp.path(), options, &BulwarkHandle::new_stub()).unwrap_or_else(|e| {
            let index_dir = tmp.path().join(".brv").join("index");
            let state = read_state(&index_dir).ok();
            panic!(
                "M4 iteration {}: curate failed: {}\nState: {:?}",
                i, e, state
            );
        });

        assert!(
            result.sync_compile_result.is_some(),
            "M4 iteration {}: sync_compile_result was None",
            i
        );

        // Query for the unique term
        let index_dir = tmp.path().join(".brv").join("index").join("tantivy");
        let index = Index::open_in_dir(&index_dir).unwrap_or_else(|e| {
            panic!("M4 iteration {}: failed to open index: {}", i, e);
        });
        let reader = index.reader().unwrap();
        let searcher = reader.searcher();

        let body_field = schema.get_field("body").unwrap();
        let title_field = schema.get_field("title").unwrap();
        let qp = QueryParser::for_index(&index, vec![title_field, body_field]);
        let query = qp.parse_query(&unique_term).unwrap();

        let top_docs = searcher.search(&query, &TopDocs::with_limit(10)).unwrap();
        if top_docs.is_empty() {
            let index_dir_state = tmp.path().join(".brv").join("index");
            let state = read_state(&index_dir_state).ok();
            panic!(
                "M4 iteration {}: query for '{}' returned no results.\nState: {:?}",
                i, unique_term, state
            );
        }
    }
}
