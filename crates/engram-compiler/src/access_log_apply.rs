use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;
use tantivy::schema::Field;
use tantivy::{DocAddress, Index, ReloadPolicy};

use engram_core::FactRecord;

/// Mirror of `engram_query::access_log::AccessLogEntry` for deserialization.
/// Duplicated here to avoid a circular dependency (engram-query depends on
/// engram-compiler in dev-dependencies).
#[derive(Debug, Deserialize)]
struct AccessLogEntry {
    #[allow(dead_code)]
    ts: i64,
    fact_id: String,
    #[allow(dead_code)]
    agent: String,
    gen: u64,
}

/// Read the access log and return a tally of access counts per fact_id.
///
/// Skips malformed lines and entries from stale generations (older than
/// `current_gen - 1`). Returns an empty map if the log doesn't exist or is empty.
pub fn tally_access_log(
    log_path: &Path,
    current_gen: u64,
) -> HashMap<String, u64> {
    let mut counts = HashMap::new();

    let content = match std::fs::read_to_string(log_path) {
        Ok(c) if !c.trim().is_empty() => c,
        _ => return counts,
    };

    for line in content.lines() {
        let entry: AccessLogEntry = match serde_json::from_str(line) {
            Ok(e) => e,
            Err(_) => continue,
        };
        if current_gen > 1 && entry.gen + 1 < current_gen {
            continue;
        }
        *counts.entry(entry.fact_id).or_insert(0) += 1;
    }

    counts
}

/// Read existing access counts from the committed Tantivy index.
///
/// Opens a read-only snapshot of the last committed index state and reads
/// the `access_count` FAST field for each record by matching on
/// `source_path_hash`. Returns a map of `fact_id → previous_count`.
///
/// Entirely non-fatal: returns an empty map on any error (first compile,
/// missing fields, I/O errors). Never causes a compile failure.
pub fn read_existing_access_counts(
    index: &Index,
    records: &[FactRecord],
    f_source_path_hash: Option<Field>,
    f_access_count: Field,
) -> HashMap<String, u64> {
    let f_sph = match f_source_path_hash {
        Some(f) => f,
        None => return HashMap::new(), // pre-v3 schema
    };

    let reader = match index.reader_builder().reload_policy(ReloadPolicy::Manual).try_into() {
        Ok(r) => r,
        Err(_) => {
            eprintln!("DEBUG: no committed index yet — skipping previous access count read");
            return HashMap::new();
        }
    };

    let searcher = reader.searcher();

    // Build hash → DocAddress map from the FAST column (same logic as
    // engram-query's build_doc_address_map, reimplemented inline per NRD-5).
    let mut hash_to_doc: HashMap<u64, DocAddress> = HashMap::new();
    for (segment_ord, segment) in searcher.segment_readers().iter().enumerate() {
        let fast = segment.fast_fields();
        let col = match fast.u64(index.schema().get_field_name(f_sph)) {
            Ok(c) => c,
            Err(_) => return HashMap::new(), // FAST column missing
        };

        let alive = segment.alive_bitset();
        for doc_id in 0..segment.max_doc() {
            if let Some(bitset) = &alive {
                if !bitset.is_alive(doc_id) {
                    continue;
                }
            }
            if let Some(hash) = col.first(doc_id) {
                hash_to_doc.insert(hash, DocAddress::new(segment_ord as u32, doc_id));
            }
        }
    }

    // For each record, look up the previous access_count
    let schema = index.schema();
    let access_count_name = schema.get_field_name(f_access_count);
    let mut previous_counts = HashMap::new();

    for record in records {
        let sp = record.source_path.to_string_lossy();
        let hash = engram_core::hash::fnv1a_u64(sp.as_bytes());

        if let Some(&addr) = hash_to_doc.get(&hash) {
            let segment = searcher.segment_reader(addr.segment_ord);
            let fast = segment.fast_fields();
            if let Ok(col) = fast.u64(access_count_name) {
                if let Some(count) = col.first(addr.doc_id) {
                    if count > 0 {
                        previous_counts.insert(record.id.clone(), count);
                    }
                }
            }
        }
    }

    previous_counts
}

/// Apply access counts to FactRecords before they are written to Tantivy.
///
/// For each record, computes `total = previous_counts[id] + current_tally[id]`.
/// Sets `record.access_count = total` and bumps `importance` by
/// `importance_delta × current_tally[id]` (only the new accesses affect importance).
///
/// Records are modified in-place.
pub fn apply_access_counts(
    records: &mut [FactRecord],
    current_tally: &HashMap<String, u64>,
    previous_counts: &HashMap<String, u64>,
    importance_delta: f64,
) -> ApplyStats {
    let mut facts_updated = 0u64;

    for record in records.iter_mut() {
        let prev = previous_counts.get(&record.id).copied().unwrap_or(0);
        let current = current_tally.get(&record.id).copied().unwrap_or(0);
        let total = prev + current;

        if total > 0 {
            record.access_count = total;
            record.importance += importance_delta * (current as f64);
            facts_updated += 1;
        }
    }

    ApplyStats { facts_updated }
}

/// Truncate the access log after a successful compile.
pub fn truncate_access_log(log_path: &Path) {
    if log_path.exists() {
        if let Err(e) = std::fs::write(log_path, b"") {
            eprintln!("WARN: failed to truncate access log: {}", e);
        }
    }
}

#[derive(Debug, Default)]
pub struct ApplyStats {
    pub facts_updated: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tally_empty_log() {
        let tmp = tempfile::tempdir().unwrap();
        let log_path = tmp.path().join("access.log");
        let counts = tally_access_log(&log_path, 1);
        assert!(counts.is_empty());
    }

    #[test]
    fn test_tally_nonexistent_log() {
        let counts = tally_access_log(Path::new("/nonexistent/access.log"), 1);
        assert!(counts.is_empty());
    }

    #[test]
    fn test_tally_counts_correctly() {
        let tmp = tempfile::tempdir().unwrap();
        let log_path = tmp.path().join("access.log");
        let content = r#"{"ts":1000,"fact_id":"a","agent":"test","gen":1}
{"ts":1001,"fact_id":"b","agent":"test","gen":1}
{"ts":1002,"fact_id":"a","agent":"test","gen":1}
"#;
        std::fs::write(&log_path, content).unwrap();

        let counts = tally_access_log(&log_path, 2);
        assert_eq!(counts.get("a"), Some(&2));
        assert_eq!(counts.get("b"), Some(&1));
    }

    #[test]
    fn test_tally_skips_stale_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let log_path = tmp.path().join("access.log");
        let content = r#"{"ts":1000,"fact_id":"old","agent":"test","gen":1}
{"ts":1001,"fact_id":"current","agent":"test","gen":5}
"#;
        std::fs::write(&log_path, content).unwrap();

        // current_gen=6: gen 1 is stale (1+1 < 6), gen 5 is not (5+1 >= 6)
        let counts = tally_access_log(&log_path, 6);
        assert!(!counts.contains_key("old"));
        assert_eq!(counts.get("current"), Some(&1));
    }

    #[test]
    fn test_tally_skips_malformed_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let log_path = tmp.path().join("access.log");
        let content = "not json\n{\"ts\":1000,\"fact_id\":\"ok\",\"agent\":\"t\",\"gen\":1}\n";
        std::fs::write(&log_path, content).unwrap();

        let counts = tally_access_log(&log_path, 2);
        assert_eq!(counts.get("ok"), Some(&1));
        assert_eq!(counts.len(), 1);
    }

    #[test]
    fn test_apply_access_counts_with_previous() {
        use engram_core::FactType;
        use std::path::PathBuf;

        let mut records = vec![
            FactRecord {
                id: "fact-a".to_string(),
                source_path: PathBuf::from("a.md"),
                title: Some("A".to_string()),
                tags: vec![],
                keywords: vec![],
                related: vec![],
                importance: 0.5,
                recency: 0.5,
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
                body: "body".to_string(),
                fact_type_explicit: true,
                warnings: vec![],
            },
        ];

        let mut current_tally = HashMap::new();
        current_tally.insert("fact-a".to_string(), 3u64);

        let mut previous_counts = HashMap::new();
        previous_counts.insert("fact-a".to_string(), 5u64);

        let stats = apply_access_counts(
            &mut records,
            &current_tally,
            &previous_counts,
            0.001,
        );

        assert_eq!(stats.facts_updated, 1);
        // importance bumped only by current tally (3 * 0.001 = 0.003)
        assert!((records[0].importance - 0.503).abs() < 1e-10);
        // access_count = previous (5) + current (3) = 8
        assert_eq!(records[0].access_count, 8);
    }

    #[test]
    fn test_apply_access_counts_previous_only() {
        use engram_core::FactType;
        use std::path::PathBuf;

        let mut records = vec![
            FactRecord {
                id: "fact-b".to_string(),
                source_path: PathBuf::from("b.md"),
                title: Some("B".to_string()),
                tags: vec![],
                keywords: vec![],
                related: vec![],
                importance: 0.5,
                recency: 0.5,
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
                body: "body".to_string(),
                fact_type_explicit: true,
                warnings: vec![],
            },
        ];

        let current_tally = HashMap::new(); // no new accesses

        let mut previous_counts = HashMap::new();
        previous_counts.insert("fact-b".to_string(), 7u64);

        let stats = apply_access_counts(
            &mut records,
            &current_tally,
            &previous_counts,
            0.001,
        );

        assert_eq!(stats.facts_updated, 1);
        // importance not bumped (no current accesses)
        assert!((records[0].importance - 0.5).abs() < 1e-10);
        // access_count carries forward previous count
        assert_eq!(records[0].access_count, 7);
    }
}
