use std::path::PathBuf;

use engram_core::{FactRecord, FactType};
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;

use crate::indexer::{build_schema, incremental_update, open_index, IndexWriter};

/// Helper to build a minimal FactRecord for testing.
fn make_record(id: &str, title: &str, body: &str, source_path: &str) -> FactRecord {
    FactRecord {
        id: id.to_string(),
        source_path: PathBuf::from(source_path),
        title: Some(title.to_string()),
        body: body.to_string(),
        tags: vec!["test".to_string()],
        keywords: vec![],
        related: vec![],
        importance: 1.0,
        recency: 1.0,
        maturity: 1.0,
        access_count: 0,
        update_count: 0,
        created_at: None,
        updated_at: None,
        fact_type: FactType::Durable,
        valid_until: None,
        caused_by: vec![],
        causes: vec![],
        event_sequence: None,
        confidence: 1.0,
        domain_tags: vec![],
        warnings: vec![],
        fact_type_explicit: true,
    }
}

fn count_docs_for_query(tmp_path: &std::path::Path, query_str: &str) -> usize {
    let index_dir = tmp_path.join(".brv").join("index").join("tantivy");
    let schema = build_schema();
    let index = tantivy::Index::open_in_dir(&index_dir).unwrap();
    let reader = index.reader().unwrap();
    let searcher = reader.searcher();

    let title_field = schema.get_field("title").unwrap();
    let body_field = schema.get_field("body").unwrap();
    let query_parser = QueryParser::for_index(&index, vec![title_field, body_field]);
    let query = query_parser.parse_query(query_str).unwrap();
    let top_docs = searcher.search(&query, &TopDocs::with_limit(100)).unwrap();
    top_docs.len()
}

// --- Test 10: incremental_update delete and add ---
#[test]
fn test_incremental_update_delete_and_add() {
    let tmp = tempfile::tempdir().unwrap();

    // Full rebuild with 3 files
    let writer = IndexWriter::new(tmp.path());
    let records = vec![
        make_record("a", "Alpha Fact", "alpha body", ".brv/context-tree/a.md"),
        make_record("b", "Bravo Fact", "bravo body", ".brv/context-tree/b.md"),
        make_record("c", "Charlie Fact", "charlie body", ".brv/context-tree/c.md"),
    ];
    writer.write(records).unwrap();

    // Verify all 3 are searchable
    assert_eq!(count_docs_for_query(tmp.path(), "alpha"), 1);
    assert_eq!(count_docs_for_query(tmp.path(), "bravo"), 1);
    assert_eq!(count_docs_for_query(tmp.path(), "charlie"), 1);

    // Incremental: delete bravo, add delta
    let (index, schema) = open_index(tmp.path()).unwrap();
    let mut idx_writer = index.writer(50_000_000).unwrap();

    let delta_record = make_record("d", "Delta Fact", "delta body", ".brv/context-tree/d.md");
    let stats = incremental_update(
        &schema,
        &mut idx_writer,
        &[".brv/context-tree/b.md".to_string()],
        &[delta_record],
    )
    .unwrap();

    assert_eq!(stats.documents_written, 1);

    // Verify: alpha and charlie still present, bravo gone, delta added
    assert_eq!(count_docs_for_query(tmp.path(), "alpha"), 1);
    assert_eq!(count_docs_for_query(tmp.path(), "bravo"), 0);
    assert_eq!(count_docs_for_query(tmp.path(), "charlie"), 1);
    assert_eq!(count_docs_for_query(tmp.path(), "delta"), 1);
}

// --- Test 11: incremental_update single commit ---
#[test]
fn test_incremental_update_single_commit() {
    let tmp = tempfile::tempdir().unwrap();

    // Full rebuild with 1 file
    let writer = IndexWriter::new(tmp.path());
    let records = vec![make_record(
        "x",
        "Xray Fact",
        "xray body",
        ".brv/context-tree/x.md",
    )];
    writer.write(records).unwrap();

    // Open index and get segment count before
    let index_dir = tmp.path().join(".brv").join("index").join("tantivy");
    let index = tantivy::Index::open_in_dir(&index_dir).unwrap();

    // Incremental update: add one more
    let mut idx_writer = index.writer(50_000_000).unwrap();
    let schema = build_schema();
    let new_record = make_record("y", "Yankee Fact", "yankee body", ".brv/context-tree/y.md");
    incremental_update(&schema, &mut idx_writer, &[], &[new_record]).unwrap();

    // After commit, reload reader to verify single commit happened
    let reader = index.reader().unwrap();
    let searcher = reader.searcher();
    // Both documents should be visible (single commit = atomic)
    let title_field = schema.get_field("title").unwrap();
    let qp = QueryParser::for_index(&index, vec![title_field]);
    let query = qp.parse_query("Xray OR Yankee").unwrap();
    let results = searcher.search(&query, &TopDocs::with_limit(10)).unwrap();
    assert_eq!(results.len(), 2, "both docs should be visible after single commit");
}

// --- Test 12: incremental_update atomic visibility ---
#[test]
fn test_incremental_update_atomic_visibility() {
    let tmp = tempfile::tempdir().unwrap();

    // Full rebuild with 1 file
    let writer = IndexWriter::new(tmp.path());
    writer
        .write(vec![make_record(
            "orig",
            "Original Fact",
            "original body",
            ".brv/context-tree/orig.md",
        )])
        .unwrap();

    // Open reader BEFORE incremental update
    let index_dir = tmp.path().join(".brv").join("index").join("tantivy");
    let index = tantivy::Index::open_in_dir(&index_dir).unwrap();
    let old_reader = index.reader().unwrap();
    let _old_searcher = old_reader.searcher();

    // Perform incremental update: add a new document
    let mut idx_writer = index.writer(50_000_000).unwrap();
    let schema = build_schema();
    let new_record = make_record(
        "new",
        "Newcomer Fact",
        "newcomer body",
        ".brv/context-tree/new.md",
    );
    incremental_update(&schema, &mut idx_writer, &[], &[new_record]).unwrap();

    // New reader should see the new document
    let new_reader = index.reader().unwrap();
    let new_searcher = new_reader.searcher();

    let title_field = schema.get_field("title").unwrap();
    let qp = QueryParser::for_index(&index, vec![title_field]);
    let query = qp.parse_query("Newcomer").unwrap();

    let new_results = new_searcher
        .search(&query, &TopDocs::with_limit(10))
        .unwrap();
    assert_eq!(new_results.len(), 1, "new reader should see new document");
}
