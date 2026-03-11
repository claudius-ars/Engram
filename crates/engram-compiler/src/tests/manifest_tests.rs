use std::path::PathBuf;

use engram_core::{FactRecord, FactType};

use crate::indexer::NULL_TIMESTAMP;
use crate::manifest::ManifestWriter;

fn make_test_record(id: &str, importance: f64, confidence: f64) -> FactRecord {
    FactRecord {
        id: id.to_string(),
        source_path: PathBuf::from(format!("test/{}.md", id)),
        title: Some(format!("Title {}", id)),
        body: String::new(),
        tags: vec![],
        keywords: vec![],
        related: vec![],
        importance,
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
        confidence,
        domain_tags: vec![],
        warnings: vec![],
    }
}

// --- Test 6: manifest write/read roundtrip ---
#[test]
fn test_manifest_write_read_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let writer = ManifestWriter::new(tmp.path());

    let records = vec![
        make_test_record("a", 0.8, 0.9),
        make_test_record("b", 1.0, 1.0),
        make_test_record("c", 0.5, 0.7),
    ];

    let stats = writer.write(&records).unwrap();
    assert_eq!(stats.entries_written, 3);

    let entries = ManifestWriter::read(tmp.path()).unwrap();
    assert_eq!(entries.len(), 3);

    assert_eq!(entries[0].id, "a");
    assert_eq!(entries[0].importance, 0.8);
    assert_eq!(entries[0].confidence, 0.9);
    assert_eq!(entries[0].valid_until_ts, NULL_TIMESTAMP);
    assert_eq!(entries[0].updated_at_ts, NULL_TIMESTAMP);

    assert_eq!(entries[1].id, "b");
    assert_eq!(entries[2].id, "c");
}

// --- Test 7: atomic write leaves no .tmp file ---
#[test]
fn test_manifest_atomic_write() {
    let tmp = tempfile::tempdir().unwrap();
    let writer = ManifestWriter::new(tmp.path());

    let records = vec![make_test_record("a", 1.0, 1.0)];
    writer.write(&records).unwrap();

    let tmp_path = tmp
        .path()
        .join(".brv")
        .join("index")
        .join("manifest.bin.tmp");
    assert!(
        !tmp_path.exists(),
        ".tmp file should not exist after write"
    );
}

// --- Test 8: manifest entry from FactRecord ---
#[test]
fn test_manifest_entry_from_record() {
    let tmp = tempfile::tempdir().unwrap();
    let writer = ManifestWriter::new(tmp.path());

    let mut record = make_test_record("infra/k8s", 0.85, 0.92);
    record.fact_type = FactType::State;

    writer.write(&[record]).unwrap();

    let entries = ManifestWriter::read(tmp.path()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id, "infra/k8s");
    assert_eq!(entries[0].fact_type, 1); // State = 1
    assert_eq!(entries[0].importance, 0.85);
    assert_eq!(entries[0].confidence, 0.92);
}

// --- Test 9: empty manifest ---
#[test]
fn test_manifest_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let writer = ManifestWriter::new(tmp.path());

    let stats = writer.write(&[]).unwrap();
    assert_eq!(stats.entries_written, 0);

    let entries = ManifestWriter::read(tmp.path()).unwrap();
    assert!(entries.is_empty());
}

// --- Test 10: manifest size_bytes matches file ---
#[test]
fn test_manifest_size_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    let writer = ManifestWriter::new(tmp.path());

    let records: Vec<FactRecord> = (0..5)
        .map(|i| make_test_record(&format!("fact{}", i), 0.5 + i as f64 * 0.1, 1.0))
        .collect();

    let stats = writer.write(&records).unwrap();
    assert!(stats.size_bytes > 0);

    let file_size = std::fs::metadata(
        tmp.path()
            .join(".brv")
            .join("index")
            .join("manifest.bin"),
    )
    .unwrap()
    .len();
    assert_eq!(stats.size_bytes, file_size);
}
