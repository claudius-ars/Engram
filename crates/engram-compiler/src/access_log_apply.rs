use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

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

/// Apply access counts to FactRecords before they are written to Tantivy.
///
/// For each record whose `id` appears in `counts`:
/// - `importance` is increased by `importance_delta × count`
/// - `access_count` is increased by `count`
///
/// Records are modified in-place.
pub fn apply_access_counts(
    records: &mut [FactRecord],
    counts: &HashMap<String, u64>,
    importance_delta: f64,
) -> ApplyStats {
    let mut facts_updated = 0u64;

    for record in records.iter_mut() {
        if let Some(&count) = counts.get(&record.id) {
            record.importance += importance_delta * (count as f64);
            record.access_count += count;
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
    fn test_apply_access_counts() {
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

        let mut counts = HashMap::new();
        counts.insert("fact-a".to_string(), 3u64);

        let stats = apply_access_counts(&mut records, &counts, 0.001);

        assert_eq!(stats.facts_updated, 1);
        assert!((records[0].importance - 0.503).abs() < 1e-10);
        assert_eq!(records[0].access_count, 3);
    }
}
