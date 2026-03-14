use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::result::QueryHit;

/// A single access log entry, serialized as NDJSON.
#[derive(Debug, Serialize, Deserialize)]
pub struct AccessLogEntry {
    pub ts: i64,
    pub fact_id: String,
    pub agent: String,
    pub gen: u64,
}

/// Appends one access log entry per QueryHit to the access log.
///
/// Non-blocking: single write() call per entry, no fsync.
/// Non-fatal: any error is logged via eprintln, function returns normally.
pub fn append_access_entries(
    log_path: &Path,
    hits: &[QueryHit],
    agent_id: &str,
    generation: u64,
) {
    if hits.is_empty() {
        return;
    }

    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path);

    let mut file = match file {
        Ok(f) => f,
        Err(e) => {
            eprintln!("WARN: access_log: cannot open {:?}: {}", log_path, e);
            return;
        }
    };

    let now = chrono::Utc::now().timestamp();

    for hit in hits {
        // Skip synthetic hits (LLM, causal sentinels, temporal sentinels)
        if hit.id.is_empty() || hit.id == "llm-synthesized" {
            continue;
        }
        let entry = AccessLogEntry {
            ts: now,
            fact_id: hit.id.clone(),
            agent: agent_id.to_owned(),
            gen: generation,
        };
        let mut line = match serde_json::to_string(&entry) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("WARN: access_log: serialize failed for '{}': {}", hit.id, e);
                continue;
            }
        };
        line.push('\n');
        if let Err(e) = file.write_all(line.as_bytes()) {
            eprintln!("WARN: access_log: write failed: {}", e);
            return; // stop trying after first write failure
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_access_log_entry_serialization() {
        let entry = AccessLogEntry {
            ts: 1700000000,
            fact_id: "my-fact-id".to_string(),
            agent: "test-agent".to_string(),
            gen: 42,
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"ts\":1700000000"));
        assert!(json.contains("\"fact_id\":\"my-fact-id\""));
        assert!(json.contains("\"agent\":\"test-agent\""));
        assert!(json.contains("\"gen\":42"));

        // Roundtrip
        let parsed: AccessLogEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.ts, 1700000000);
        assert_eq!(parsed.fact_id, "my-fact-id");
        assert_eq!(parsed.agent, "test-agent");
        assert_eq!(parsed.gen, 42);
    }

    #[test]
    fn test_append_skips_empty_hits() {
        let tmp = tempfile::tempdir().unwrap();
        let log_path = tmp.path().join("access.log");

        append_access_entries(&log_path, &[], "agent", 1);

        // File should not be created when there are no hits
        assert!(!log_path.exists());
    }

    #[test]
    fn test_append_skips_synthetic_hits() {
        let tmp = tempfile::tempdir().unwrap();
        let log_path = tmp.path().join("access.log");

        let hits = vec![
            QueryHit {
                id: "".to_string(),
                title: None,
                source_path: String::new(),
                tags: vec![],
                domain_tags: vec![],
                score: 0.0,
                bm25_score: 0.0,
                fact_type: "durable".to_string(),
                confidence: 1.0,
                importance: 0.5,
                recency: 0.5,
                caused_by: vec![],
                causes: vec![],
                keywords: vec![],
                related: vec![],
                maturity: 1.0,
                access_count: 0,
                update_count: 0,
                answer: None,
            },
            QueryHit {
                id: "llm-synthesized".to_string(),
                title: None,
                source_path: String::new(),
                tags: vec![],
                domain_tags: vec![],
                score: 0.0,
                bm25_score: 0.0,
                fact_type: "durable".to_string(),
                confidence: 1.0,
                importance: 0.5,
                recency: 0.5,
                caused_by: vec![],
                causes: vec![],
                keywords: vec![],
                related: vec![],
                maturity: 1.0,
                access_count: 0,
                update_count: 0,
                answer: None,
            },
        ];

        append_access_entries(&log_path, &hits, "agent", 1);

        // File is created (OpenOptions::create) but should be empty
        if log_path.exists() {
            let content = std::fs::read_to_string(&log_path).unwrap();
            assert!(content.is_empty(), "synthetic hits should not produce log entries");
        }
    }

    #[test]
    fn test_append_writes_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let log_path = tmp.path().join("access.log");

        let hits = vec![QueryHit {
            id: "test-fact".to_string(),
            title: Some("Test".to_string()),
            source_path: "test.md".to_string(),
            tags: vec![],
            domain_tags: vec![],
            score: 1.0,
            bm25_score: 0.5,
            fact_type: "durable".to_string(),
            confidence: 1.0,
            importance: 0.8,
            recency: 0.9,
            caused_by: vec![],
            causes: vec![],
            keywords: vec![],
            related: vec![],
            maturity: 1.0,
            access_count: 0,
            update_count: 0,
            answer: None,
        }];

        append_access_entries(&log_path, &hits, "my-agent", 5);

        let content = std::fs::read_to_string(&log_path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 1);

        let entry: AccessLogEntry = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(entry.fact_id, "test-fact");
        assert_eq!(entry.agent, "my-agent");
        assert_eq!(entry.gen, 5);
    }

    #[test]
    fn test_append_nonfatal_on_bad_path() {
        // Writing to a directory that doesn't exist should not panic
        let bad_path = std::path::Path::new("/nonexistent/dir/access.log");
        let hits = vec![QueryHit {
            id: "test".to_string(),
            title: None,
            source_path: String::new(),
            tags: vec![],
            domain_tags: vec![],
            score: 0.0,
            bm25_score: 0.0,
            fact_type: "durable".to_string(),
            confidence: 1.0,
            importance: 0.5,
            recency: 0.5,
            caused_by: vec![],
            causes: vec![],
            keywords: vec![],
            related: vec![],
            maturity: 1.0,
            access_count: 0,
            update_count: 0,
            answer: None,
        }];

        // Should not panic — just prints a WARN
        append_access_entries(bad_path, &hits, "agent", 1);
    }
}
