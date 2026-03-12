use std::collections::HashSet;

use engram_core::temporal::{
    fnv1a_64, TemporalLogHeader, TemporalRecord, EVENT_KIND_CREATED, FACT_TYPE_DURABLE,
    FACT_TYPE_STATE, NULL_TIMESTAMP, TEMPORAL_MAGIC, TEMPORAL_VERSION,
};

use crate::temporal_query::{
    classify_temporal_query, has_temporal_signal, merge_temporal_and_bm25,
    temporal_record_to_query_hit, TemporalQueryPattern,
};
use crate::temporal_reader::TemporalReader;

/// Helper to build raw temporal.log bytes from header + records.
fn build_log(header: &TemporalLogHeader, records: &[TemporalRecord]) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(bytemuck::bytes_of(header));
    buf.extend_from_slice(bytemuck::cast_slice(records));
    buf
}

fn make_header(record_count: u32, generation: u64) -> TemporalLogHeader {
    TemporalLogHeader {
        magic: TEMPORAL_MAGIC,
        version: TEMPORAL_VERSION,
        record_count,
        compiled_at_ts: 10000,
        generation,
        _pad: [0u8; 32],
    }
}

fn make_record(
    event_ts: i64,
    fact_type: u8,
    event_kind: u8,
    valid_until_ts: i64,
    source_path_hash: u64,
) -> TemporalRecord {
    TemporalRecord {
        event_ts,
        valid_until_ts,
        created_at_ts: event_ts,
        source_path_hash,
        content_hash: [0u8; 16],
        fact_type,
        event_kind,
        _pad: [0u8; 14],
    }
}

// --- Test 1: has_temporal_signal positive ---
#[test]
fn test_has_temporal_signal_positive() {
    assert!(has_temporal_signal("what is the current state"));
    assert!(has_temporal_signal("what changed since last week"));
    assert!(has_temporal_signal("show me the history of deployments"));
    assert!(has_temporal_signal("when did the migration happen"));
    assert!(has_temporal_signal("what is the latest version"));
}

// --- Test 2: has_temporal_signal negative ---
#[test]
fn test_has_temporal_signal_negative() {
    assert!(!has_temporal_signal("what is the retry policy"));
    assert!(!has_temporal_signal("explain authentication flow"));
}

// --- Test 3: word boundary matching ---
#[test]
fn test_word_boundary_matching() {
    // "was" inside "password" must NOT trigger
    assert!(
        !has_temporal_signal("password reset policy"),
        "\"was\" inside \"password\" should not trigger"
    );
    // "was" as a standalone word DOES trigger
    assert!(
        has_temporal_signal("what was the policy"),
        "\"was\" as standalone word should trigger"
    );
}

// --- Test 4: classify current state ---
#[test]
fn test_classify_current_state() {
    let pattern = classify_temporal_query("what is the current state of the auth service");
    assert_eq!(pattern, TemporalQueryPattern::CurrentState);
}

// --- Test 5: classify since timestamp ---
#[test]
fn test_classify_since_timestamp() {
    let pattern = classify_temporal_query("what changed since 2024");
    match pattern {
        TemporalQueryPattern::SinceTimestamp(ts) => {
            // Jan 1 2024 00:00:00 UTC = 1704067200
            assert_eq!(ts, 1704067200, "should parse 2024 to Jan 1 2024 UTC");
        }
        other => panic!("expected SinceTimestamp, got {:?}", other),
    }
}

// --- Test 6: classify since no year ---
#[test]
fn test_classify_since_no_year() {
    let pattern = classify_temporal_query("what changed since the migration");
    match pattern {
        TemporalQueryPattern::SinceTimestamp(ts) => {
            assert_eq!(ts, i64::MIN, "no year should return i64::MIN");
        }
        other => panic!("expected SinceTimestamp(i64::MIN), got {:?}", other),
    }
}

// --- Test 7: classify event history ---
#[test]
fn test_classify_event_history() {
    let pattern = classify_temporal_query("show me the history of the deployment config");
    assert_eq!(pattern, TemporalQueryPattern::EventHistory);
}

// --- Test 8: tier2_5 excludes expired state ---
#[test]
fn test_tier2_5_excludes_expired_state() {
    let hash_a = fnv1a_64(b"current.md");
    let hash_b = fnv1a_64(b"expired.md");
    let now_ts = 5000i64;

    let records = vec![
        // Current state (no expiry)
        make_record(1000, FACT_TYPE_STATE, EVENT_KIND_CREATED, NULL_TIMESTAMP, hash_a),
        // Expired state (expired at 3000, now is 5000)
        make_record(2000, FACT_TYPE_STATE, EVENT_KIND_CREATED, 3000, hash_b),
    ];
    let header = make_header(records.len() as u32, 1);
    let data = build_log(&header, &records);

    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("temporal.log"), &data).unwrap();

    let reader = TemporalReader::load(tmp.path()).unwrap().unwrap();
    let results = reader.tier2_5_search(&TemporalQueryPattern::CurrentState, now_ts, 1);

    assert_eq!(results.len(), 1, "should only return non-expired state");
    assert_eq!(results[0].source_path_hash, hash_a);
}

// --- Test 9: tier2_5 stale generation returns empty ---
#[test]
fn test_tier2_5_stale_generation_returns_empty() {
    let records = vec![make_record(
        1000,
        FACT_TYPE_STATE,
        EVENT_KIND_CREATED,
        NULL_TIMESTAMP,
        fnv1a_64(b"a.md"),
    )];
    let header = make_header(records.len() as u32, 5);
    let data = build_log(&header, &records);

    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("temporal.log"), &data).unwrap();

    let reader = TemporalReader::load(tmp.path()).unwrap().unwrap();
    // Generation mismatch: log has gen 5, we pass gen 6
    let results = reader.tier2_5_search(&TemporalQueryPattern::CurrentState, 5000, 6);
    assert!(results.is_empty(), "mismatched generation should return empty");
}

// --- Test 10: tier2_5 since timestamp filters ---
#[test]
fn test_tier2_5_since_timestamp_filters() {
    let hash = fnv1a_64(b"x.md");
    let records = vec![
        make_record(100, FACT_TYPE_DURABLE, EVENT_KIND_CREATED, NULL_TIMESTAMP, hash),
        make_record(200, FACT_TYPE_DURABLE, EVENT_KIND_CREATED, NULL_TIMESTAMP, hash),
        make_record(300, FACT_TYPE_DURABLE, EVENT_KIND_CREATED, NULL_TIMESTAMP, hash),
    ];
    let header = make_header(records.len() as u32, 1);
    let data = build_log(&header, &records);

    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("temporal.log"), &data).unwrap();

    let reader = TemporalReader::load(tmp.path()).unwrap().unwrap();
    let results = reader.tier2_5_search(&TemporalQueryPattern::SinceTimestamp(200), 5000, 1);

    assert_eq!(results.len(), 2, "should return records at t=200 and t=300");
    assert_eq!(results[0].event_ts, 200);
    assert_eq!(results[1].event_ts, 300);
}

// --- Test 11: merge deduplicates by source_path_hash ---
#[test]
fn test_merge_deduplicates_by_source_path_hash() {
    let hash_a = fnv1a_64(b"shared.md");

    // Temporal hit for shared.md
    let temporal_record = make_record(
        1000,
        FACT_TYPE_STATE,
        EVENT_KIND_CREATED,
        NULL_TIMESTAMP,
        hash_a,
    );
    let temporal_hit = temporal_record_to_query_hit(&temporal_record);
    let temporal_hashes: HashSet<u64> = [hash_a].into_iter().collect();

    // BM25 hit for the same file
    let bm25_hit = crate::result::QueryHit {
        id: "bm25-1".to_string(),
        title: Some("Shared Fact".to_string()),
        source_path: "shared.md".to_string(),
        tags: vec![],
        domain_tags: vec![],
        score: 0.8,
        bm25_score: 0.8,
        fact_type: "state".to_string(),
        confidence: 1.0,
        importance: 1.0,
        recency: 1.0,
        caused_by: vec![],
        causes: vec![],
        keywords: vec![],
        related: vec![],
        maturity: 1.0,
        access_count: 0,
        update_count: 0,
    };

    // BM25 hit for a different file
    let bm25_hit_other = crate::result::QueryHit {
        id: "bm25-2".to_string(),
        title: Some("Other Fact".to_string()),
        source_path: "other.md".to_string(),
        tags: vec![],
        domain_tags: vec![],
        score: 0.7,
        bm25_score: 0.7,
        fact_type: "durable".to_string(),
        confidence: 1.0,
        importance: 1.0,
        recency: 1.0,
        caused_by: vec![],
        causes: vec![],
        keywords: vec![],
        related: vec![],
        maturity: 1.0,
        access_count: 0,
        update_count: 0,
    };

    let merged = merge_temporal_and_bm25(
        vec![temporal_hit],
        &temporal_hashes,
        vec![bm25_hit, bm25_hit_other],
    );

    // Should have 2 hits: temporal version of shared.md + other.md
    assert_eq!(merged.len(), 2, "should deduplicate shared.md, keep other.md");
    // First hit should be temporal (score = 1.0)
    assert_eq!(merged[0].score, 1.0, "first hit should be temporal");
    // Second hit should be the non-duplicate BM25 hit
    assert_eq!(merged[1].source_path, "other.md");
}
