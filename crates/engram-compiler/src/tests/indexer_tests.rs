use std::path::PathBuf;

use engram_core::{FactRecord, FactType};
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::Value;
use tantivy::{Index, TantivyDocument};

use engram_bulwark::BulwarkHandle;

use crate::indexer::{build_schema, IndexWriter, CURRENT_SCHEMA_VERSION};

/// Helper to build a minimal FactRecord for testing.
fn make_record(id: &str, title: &str, body: &str) -> FactRecord {
    FactRecord {
        id: id.to_string(),
        source_path: PathBuf::from(format!("test/{}.md", id)),
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

// --- Test 1: schema version file written ---
#[test]
fn test_schema_version_written() {
    let tmp = tempfile::tempdir().unwrap();
    let writer = IndexWriter::new(tmp.path());
    let records = vec![make_record("test/a", "Test A", "body text")];
    writer.write(records).unwrap();

    let version_path = tmp
        .path()
        .join(".brv")
        .join("index")
        .join("tantivy")
        .join("engram_schema_version");
    assert!(version_path.exists());
    let content = std::fs::read_to_string(&version_path).unwrap();
    assert_eq!(content.trim(), CURRENT_SCHEMA_VERSION.to_string());
}

// --- Test 2: index directory is non-empty after write ---
#[test]
fn test_index_written() {
    let tmp = tempfile::tempdir().unwrap();
    let writer = IndexWriter::new(tmp.path());
    let records = vec![
        make_record("legacy", "Legacy Fact", "legacy body"),
        make_record("engram", "Engram Fact", "engram body"),
    ];
    let stats = writer.write(records).unwrap();
    assert_eq!(stats.documents_written, 2);
    assert_eq!(stats.documents_skipped, 0);

    let index_dir = tmp.path().join(".brv").join("index").join("tantivy");
    assert!(index_dir.exists());
    let entries: Vec<_> = std::fs::read_dir(&index_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    // At minimum: schema version file + tantivy meta + segment files
    assert!(entries.len() > 1);
}

// --- Test 3: index is searchable after write ---
#[test]
fn test_index_searchable() {
    let tmp = tempfile::tempdir().unwrap();
    let writer = IndexWriter::new(tmp.path());
    let records = vec![
        make_record("legacy", "Legacy Fact", "This is a legacy document"),
        make_record("engram", "Engram Fact", "This is an engram document"),
    ];
    writer.write(records).unwrap();

    let index_dir = tmp.path().join(".brv").join("index").join("tantivy");
    let schema = build_schema();
    let index = Index::open_in_dir(&index_dir).unwrap();
    let reader = index.reader().unwrap();
    let searcher = reader.searcher();

    let title_field = schema.get_field("title").unwrap();
    let query_parser = QueryParser::for_index(&index, vec![title_field]);
    let query = query_parser.parse_query("Legacy").unwrap();

    let top_docs = searcher.search(&query, &TopDocs::with_limit(10)).unwrap();
    assert!(!top_docs.is_empty(), "expected at least one search result for 'Legacy'");

    // Verify the retrieved document has the correct title
    let doc: TantivyDocument = searcher.doc(top_docs[0].1).unwrap();
    let title_value = doc.get_first(title_field).unwrap();
    let title_text = title_value.as_str().unwrap();
    assert_eq!(title_text, "Legacy Fact");
}

// --- Test 4: null sentinel fields handled correctly ---
#[test]
fn test_null_sentinel_fields() {
    let tmp = tempfile::tempdir().unwrap();
    let writer = IndexWriter::new(tmp.path());

    let record = FactRecord {
        id: "null-test".to_string(),
        source_path: PathBuf::from("test/null.md"),
        title: None,
        body: String::new(),
        tags: vec![],
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
    };

    let stats = writer.write(vec![record]).unwrap();
    assert_eq!(stats.documents_written, 1);
    assert_eq!(stats.documents_skipped, 0);

    // Verify the document is retrievable
    let index_dir = tmp.path().join(".brv").join("index").join("tantivy");
    let schema = build_schema();
    let index = Index::open_in_dir(&index_dir).unwrap();
    let reader = index.reader().unwrap();
    let searcher = reader.searcher();

    let id_field = schema.get_field("id").unwrap();
    let query_parser = QueryParser::for_index(&index, vec![id_field]);
    let query = query_parser.parse_query("\"null-test\"").unwrap();
    let top_docs = searcher.search(&query, &TopDocs::with_limit(10)).unwrap();
    assert_eq!(top_docs.len(), 1);
}

// --- Test 5: schema version too old triggers rebuild ---
#[test]
fn test_schema_version_too_old() {
    let tmp = tempfile::tempdir().unwrap();
    let index_dir = tmp.path().join(".brv").join("index").join("tantivy");
    std::fs::create_dir_all(&index_dir).unwrap();
    std::fs::write(index_dir.join("engram_schema_version"), "0").unwrap();

    let writer = IndexWriter::new(tmp.path());
    let result = writer.write(vec![]);
    assert!(result.is_ok(), "old schema version should trigger rebuild, not error");
    // Version file should now contain the current version
    let version = std::fs::read_to_string(index_dir.join("engram_schema_version")).unwrap();
    assert_eq!(version.trim(), CURRENT_SCHEMA_VERSION.to_string());
}

// --- Test 6: schema version too new triggers rebuild ---
#[test]
fn test_schema_version_too_new() {
    let tmp = tempfile::tempdir().unwrap();
    let index_dir = tmp.path().join(".brv").join("index").join("tantivy");
    std::fs::create_dir_all(&index_dir).unwrap();
    std::fs::write(index_dir.join("engram_schema_version"), "99").unwrap();

    let writer = IndexWriter::new(tmp.path());
    let result = writer.write(vec![]);
    assert!(result.is_ok(), "new schema version should trigger rebuild, not error");
    // Version file should now contain the current version
    let version = std::fs::read_to_string(index_dir.join("engram_schema_version")).unwrap();
    assert_eq!(version.trim(), CURRENT_SCHEMA_VERSION.to_string());
}

// --- Test 7: empty records ---
#[test]
fn test_empty_records() {
    let tmp = tempfile::tempdir().unwrap();
    let writer = IndexWriter::new(tmp.path());
    let stats = writer.write(vec![]).unwrap();
    assert_eq!(stats.documents_written, 0);
    assert_eq!(stats.documents_skipped, 0);
}

// --- Test 8: bench_index_500_files ---
#[test]
fn bench_index_500_files() {
    let tmp = tempfile::tempdir().unwrap();
    let writer = IndexWriter::new(tmp.path());

    let records: Vec<FactRecord> = (0..500)
        .map(|i| {
            let has_fact_type = i % 2 == 0;
            let fact_type = if has_fact_type {
                match i % 6 {
                    0 => FactType::Durable,
                    2 => FactType::State,
                    4 => FactType::Event,
                    _ => FactType::Durable,
                }
            } else {
                FactType::Durable
            };

            let event_sequence = if fact_type == FactType::Event {
                Some(i as i64)
            } else {
                None
            };

            let tag_count = (i % 3) + 1;
            let tags: Vec<String> = (0..tag_count).map(|t| format!("tag{}", t)).collect();
            let importance = 0.5 + (i % 50) as f64 * 0.01;
            let confidence = if i % 4 == 0 {
                0.5 + (i % 50) as f64 * 0.01
            } else {
                1.0
            };
            let domain_tags = if i % 4 == 1 {
                vec!["domain:alpha".to_string(), "domain:beta".to_string()]
            } else {
                vec![]
            };

            FactRecord {
                id: format!("fact{:04}", i),
                source_path: PathBuf::from(format!(".brv/context-tree/fact{:04}.md", i)),
                title: Some(format!("Fact {}", i)),
                body: format!(
                    "This is the body of fact {}. It contains text for full-text indexing.",
                    i
                ),
                tags,
                keywords: vec![],
                related: vec![],
                importance,
                recency: 1.0,
                maturity: 1.0,
                access_count: 0,
                update_count: 0,
                created_at: None,
                updated_at: None,
                fact_type,
                valid_until: None,
                caused_by: vec![],
                causes: vec![],
                event_sequence,
                confidence,
                domain_tags,
                warnings: vec![],
                fact_type_explicit: true,
            }
        })
        .collect();

    let stats = writer.write(records).unwrap();

    assert_eq!(stats.documents_written, 500);
    assert_eq!(stats.documents_skipped, 0);

    eprintln!("bench_index_500_files: {}ms", stats.elapsed_ms);

    if stats.elapsed_ms > 2000 {
        eprintln!(
            "PERF WARNING: bench_index_500_files took {}ms — \
            expected under 1500ms on developer hardware. \
            This may indicate a performance regression.",
            stats.elapsed_ms
        );
    }
}

// --- Test 9: recompile produces no duplicates ---
#[test]
fn test_recompile_no_duplicates() {
    let tmp = tempfile::tempdir().unwrap();
    let writer = IndexWriter::new(tmp.path());

    let records = vec![make_record("dup-test", "Duplicate Test", "body text")];

    // Write the same record twice in separate write() calls
    writer.write(records.clone()).unwrap();
    writer.write(records).unwrap();

    // Open and search — should find exactly one result
    let index_dir = tmp.path().join(".brv").join("index").join("tantivy");
    let schema = build_schema();
    let index = Index::open_in_dir(&index_dir).unwrap();
    let reader = index.reader().unwrap();
    let searcher = reader.searcher();

    let title_field = schema.get_field("title").unwrap();
    let query_parser = QueryParser::for_index(&index, vec![title_field]);
    let query = query_parser.parse_query("Duplicate").unwrap();

    let top_docs = searcher.search(&query, &TopDocs::with_limit(100)).unwrap();
    assert_eq!(
        top_docs.len(),
        1,
        "expected exactly 1 result, got {} (duplicates present)",
        top_docs.len()
    );
}

// --- Test 11: end-to-end compile ---
#[test]
fn test_compile_command_end_to_end() {
    let tmp = tempfile::tempdir().unwrap();
    let context_tree = tmp.path().join(".brv").join("context-tree");
    std::fs::create_dir_all(&context_tree).unwrap();

    let fixtures_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures");
    std::fs::copy(
        fixtures_dir.join("valid_legacy.md"),
        context_tree.join("valid_legacy.md"),
    )
    .unwrap();
    std::fs::copy(
        fixtures_dir.join("valid_engram.md"),
        context_tree.join("valid_engram.md"),
    )
    .unwrap();

    let result = crate::compile_context_tree(tmp.path(), true, &BulwarkHandle::new_stub());

    assert_eq!(result.parse_result.records.len(), 2);

    let stats = result.index_stats.as_ref().expect("index_stats should be Some");
    assert_eq!(stats.documents_written, 2);

    let mstats = result.manifest_stats.as_ref().expect("manifest_stats should be Some");
    assert_eq!(mstats.entries_written, 2);

    let state = result.state.as_ref().expect("state should be Some");
    assert_eq!(state.generation, 1);
    assert!(!state.dirty);

    // Verify files on disk
    assert!(tmp.path().join(".brv/index/state").exists());
    assert!(tmp.path().join(".brv/index/manifest.bin").exists());
    assert!(tmp
        .path()
        .join(".brv/index/tantivy/engram_schema_version")
        .exists());

    assert!(result.index_error.is_none());
    assert!(result.manifest_error.is_none());
    assert!(result.state_error.is_none());
}

// --- Test 12: compile increments generation ---
#[test]
fn test_compile_increments_generation() {
    let tmp = tempfile::tempdir().unwrap();
    let context_tree = tmp.path().join(".brv").join("context-tree");
    std::fs::create_dir_all(&context_tree).unwrap();

    let fixtures_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures");
    std::fs::copy(
        fixtures_dir.join("valid_legacy.md"),
        context_tree.join("valid_legacy.md"),
    )
    .unwrap();

    // First compile
    let result1 = crate::compile_context_tree(tmp.path(), true, &BulwarkHandle::new_stub());
    assert_eq!(result1.state.as_ref().unwrap().generation, 1);

    // Second compile
    let result2 = crate::compile_context_tree(tmp.path(), true, &BulwarkHandle::new_stub());
    assert_eq!(result2.state.as_ref().unwrap().generation, 2);
}
