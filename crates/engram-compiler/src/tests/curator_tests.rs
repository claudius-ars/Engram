use std::path::Path;

use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::Index;

use engram_bulwark::BulwarkHandle;

use crate::compile_context_tree;
use crate::curator::{curate, make_slug, CurateError, CurateOptions};
use crate::indexer::build_schema;
use crate::state::read_state;

/// Helper to set up a temp root with context-tree and a compiled index.
fn setup_compiled_root() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let context_tree = tmp.path().join(".brv").join("context-tree");
    std::fs::create_dir_all(&context_tree).unwrap();

    // Copy a fixture so there's something to compile
    let fixtures_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures");
    std::fs::copy(
        fixtures_dir.join("valid_legacy.md"),
        context_tree.join("valid_legacy.md"),
    )
    .unwrap();

    // Run initial compile
    compile_context_tree(tmp.path(), true, &BulwarkHandle::new_stub());
    tmp
}

/// Helper to set up a temp root with context-tree but no compiled index.
fn setup_uncompiled_root() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let context_tree = tmp.path().join(".brv").join("context-tree");
    std::fs::create_dir_all(&context_tree).unwrap();
    tmp
}

// --- Test 1: slug generation ---
#[test]
fn test_slug_generation() {
    assert_eq!(
        make_slug("Rust borrow checker prevents data races at compile time").unwrap(),
        "rust-borrow-checker-prevents-data-races"
    );
    assert_eq!(
        make_slug("K8s pod scheduling uses node affinity rules").unwrap(),
        "k8s-pod-scheduling-uses-node-affinity"
    );
    assert_eq!(
        make_slug("  leading spaces and UPPERCASE  ").unwrap(),
        "leading-spaces-and-uppercase"
    );
    assert!(matches!(make_slug(""), Err(CurateError::EmptySummary)));
    assert!(matches!(make_slug("   "), Err(CurateError::EmptySummary)));
}

// --- Test 2: file written with valid frontmatter ---
#[test]
fn test_file_written() {
    let tmp = setup_compiled_root();
    let options = CurateOptions {
        summary: "Rust ownership model prevents memory leaks at zero cost".to_string(),
        sync: false,
    };

    let result = curate(tmp.path(), options, &BulwarkHandle::new_stub()).unwrap();
    assert!(result.written_path.exists());

    let content = std::fs::read_to_string(&result.written_path).unwrap();
    assert!(content.contains("Rust ownership model prevents memory leaks at zero cost"));
    assert!(content.contains("factType: durable"));
    assert!(content.contains("confidence: 1.0"));

    // Parse frontmatter to verify it's valid YAML
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines[0], "---");
    let close_idx = lines[1..].iter().position(|l| *l == "---").unwrap() + 1;
    let yaml_str: String = lines[1..close_idx].join("\n");
    let _raw: engram_core::RawFrontmatter = serde_yaml::from_str(&yaml_str).unwrap();
}

// --- Test 3: dirty flag set after async curate ---
#[test]
fn test_dirty_flag_set() {
    let tmp = setup_compiled_root();
    let index_dir = tmp.path().join(".brv").join("index");

    // Verify clean before curate
    let state_before = read_state(&index_dir).unwrap();
    assert!(!state_before.dirty);

    let options = CurateOptions {
        summary: "Test dirty flag setting".to_string(),
        sync: false,
    };
    curate(tmp.path(), options, &BulwarkHandle::new_stub()).unwrap();

    let state_after = read_state(&index_dir).unwrap();
    assert!(state_after.dirty);
    assert!(state_after.dirty_since.is_some());
}

// --- Test 4: dirty flag cleared after sync curate ---
#[test]
fn test_dirty_flag_cleared_after_sync() {
    let tmp = setup_compiled_root();
    let index_dir = tmp.path().join(".brv").join("index");

    let options = CurateOptions {
        summary: "Test sync clears dirty flag".to_string(),
        sync: true,
    };
    curate(tmp.path(), options, &BulwarkHandle::new_stub()).unwrap();

    let state = read_state(&index_dir).unwrap();
    assert!(!state.dirty);
}

// --- Test 5: sync file is queryable in Tantivy ---
#[test]
fn test_sync_file_queryable() {
    let tmp = setup_compiled_root();

    let options = CurateOptions {
        summary: "xylophone-unique-test-word in a curated fact".to_string(),
        sync: true,
    };
    curate(tmp.path(), options, &BulwarkHandle::new_stub()).unwrap();

    let index_dir = tmp.path().join(".brv").join("index").join("tantivy");
    let schema = build_schema();
    let index = Index::open_in_dir(&index_dir).unwrap();
    let reader = index.reader().unwrap();
    let searcher = reader.searcher();

    let body_field = schema.get_field("body").unwrap();
    let title_field = schema.get_field("title").unwrap();
    let qp = QueryParser::for_index(&index, vec![title_field, body_field]);
    let query = qp.parse_query("xylophone-unique-test-word").unwrap();

    let top_docs = searcher.search(&query, &TopDocs::with_limit(10)).unwrap();
    assert!(
        !top_docs.is_empty(),
        "expected curated fact to be queryable after --sync"
    );
}

// --- Test 6: unique paths on same day ---
#[test]
fn test_unique_paths_same_day() {
    let tmp = setup_compiled_root();

    let options1 = CurateOptions {
        summary: "Duplicate slug test".to_string(),
        sync: false,
    };
    let result1 = curate(tmp.path(), options1, &BulwarkHandle::new_stub()).unwrap();

    let options2 = CurateOptions {
        summary: "Duplicate slug test".to_string(),
        sync: false,
    };
    let result2 = curate(tmp.path(), options2, &BulwarkHandle::new_stub()).unwrap();

    assert_ne!(result1.written_path, result2.written_path);
    assert!(result1.written_path.exists());
    assert!(result2.written_path.exists());
}

// --- Test 7: curate without existing index ---
#[test]
fn test_curate_without_existing_index() {
    let tmp = setup_uncompiled_root();

    let options = CurateOptions {
        summary: "First fact ever curated".to_string(),
        sync: false,
    };
    let result = curate(tmp.path(), options, &BulwarkHandle::new_stub()).unwrap();

    assert!(result.written_path.exists());

    let index_dir = tmp.path().join(".brv").join("index");
    let state = read_state(&index_dir).unwrap();
    assert!(state.dirty);
    assert_eq!(state.generation, 0);
}

// --- Test 8: lock file released after sync ---
#[test]
fn test_lock_file_created_and_released() {
    let tmp = setup_compiled_root();
    let index_dir = tmp.path().join(".brv").join("index");

    let options = CurateOptions {
        summary: "Lock file test".to_string(),
        sync: true,
    };
    curate(tmp.path(), options, &BulwarkHandle::new_stub()).unwrap();

    let lock = index_dir.join("compile.lock");
    assert!(
        !lock.exists(),
        "lock file should not exist after --sync curate completes"
    );
}

// --- Test 9: sync returns compile result ---
#[test]
fn test_sync_returns_compile_result() {
    let tmp = setup_compiled_root();

    let options = CurateOptions {
        summary: "Sync compile result test".to_string(),
        sync: true,
    };
    let result = curate(tmp.path(), options, &BulwarkHandle::new_stub()).unwrap();

    let compile = result
        .sync_compile_result
        .as_ref()
        .expect("sync should return compile result");
    assert!(!compile.parse_result.records.is_empty());
    assert!(compile.index_stats.as_ref().unwrap().documents_written >= 1);
    let state = compile.state.as_ref().unwrap();
    assert!(!state.dirty);
    assert!(state.generation >= 1);
}

// --- Test 10: empty summary errors ---
#[test]
fn test_empty_summary_errors() {
    let tmp = setup_compiled_root();
    let options = CurateOptions {
        summary: "".to_string(),
        sync: false,
    };
    let result = curate(tmp.path(), options, &BulwarkHandle::new_stub());
    assert!(matches!(result, Err(CurateError::EmptySummary)));

    let options2 = CurateOptions {
        summary: "   ".to_string(),
        sync: false,
    };
    let result2 = curate(tmp.path(), options2, &BulwarkHandle::new_stub());
    assert!(matches!(result2, Err(CurateError::EmptySummary)));
}

// --- Test 11: curate with stub allowed ---
#[test]
fn test_curate_with_stub_allowed() {
    let tmp = setup_compiled_root();
    let options = CurateOptions {
        summary: "Bulwark stub allows curate".to_string(),
        sync: true,
    };
    let result = curate(tmp.path(), options, &BulwarkHandle::new_stub()).unwrap();
    assert!(result.written_path.exists());
    assert!(result.sync_compile_result.is_some());
}

// --- Test 12: compile with stub allowed ---
#[test]
fn test_compile_with_stub_allowed() {
    let tmp = setup_compiled_root();
    let result = compile_context_tree(tmp.path(), true, &BulwarkHandle::new_stub());
    assert!(result.index_error.is_none());
    assert!(result.index_stats.is_some());
}

// --- Test 13: policy denied blocks curate ---
#[test]
fn test_policy_denied_blocks_curate() {
    let tmp = setup_compiled_root();
    let options = CurateOptions {
        summary: "This should be denied".to_string(),
        sync: false,
    };
    let result = curate(tmp.path(), options, &BulwarkHandle::new_denying());
    assert!(matches!(result, Err(CurateError::PolicyDenied(_))));
}

// --- Test 14: policy denied blocks compile ---
#[test]
fn test_policy_denied_blocks_compile() {
    let tmp = setup_compiled_root();
    let result = compile_context_tree(tmp.path(), true, &BulwarkHandle::new_denying());
    assert!(result.index_error.is_some());
    match result.index_error.as_ref().unwrap() {
        crate::indexer::IndexError::PolicyDenied(_) => {}
        other => panic!("expected PolicyDenied, got: {:?}", other),
    }
}
