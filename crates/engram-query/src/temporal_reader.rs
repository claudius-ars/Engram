// ENFORCEMENT CONTRACT (valid_until):
//
// Expired state facts are handled at THREE layers:
//
// 1. Temporal log (Tier 2.5): current_state_facts() excludes records where
//    valid_until_ts < now_ts. Expired facts never appear as "currently true."
//
// 2. Compound scoring (Tier 2, Phase 2 Prompt 3): expired state facts score 0.
//    They are returned by Tantivy but scored to zero before inclusion in results.
//
// 3. Tantivy index: expired facts are NOT filtered at query time. They remain
//    fully retrievable for audit purposes.
//
// This layered approach means:
// - An agent querying "current state" never sees expired facts (Layers 1 + 2).
// - An audit query can retrieve expired facts and see that they scored 0 (Layer 3).
// - Compliance audit logs show: fact was indexed, fact expired on [date],
//   fact was considered and scored 0, fact was not returned to caller.
//
// Do NOT add a Tantivy query-time filter for valid_until. That would make
// expired facts invisible to audit, which is incorrect for regulated deployments.

use std::path::Path;

use engram_core::temporal::{
    parse_temporal_log, TemporalLogHeader, TemporalRecord, FACT_TYPE_STATE, NULL_TIMESTAMP,
};

use crate::temporal_query::TemporalQueryPattern;

pub struct TemporalReader {
    records: Vec<TemporalRecord>,
    header: TemporalLogHeader,
}

impl TemporalReader {
    /// Load temporal.log from disk. Returns Ok(None) if file does not exist.
    pub fn load(index_dir: &Path) -> anyhow::Result<Option<Self>> {
        let path = index_dir.join("temporal.log");
        if !path.exists() {
            return Ok(None);
        }

        let data = std::fs::read(&path)?;
        let (header, records_slice) = parse_temporal_log(&data)?;
        let records = records_slice.to_vec();

        Ok(Some(TemporalReader { records, header }))
    }

    /// Pattern 1: Current state facts.
    /// Returns records where fact_type == State AND
    /// (valid_until_ts == NULL_TIMESTAMP OR valid_until_ts >= now_ts).
    pub fn current_state_facts(&self, now_ts: i64) -> Vec<&TemporalRecord> {
        self.records
            .iter()
            .filter(|r| {
                r.fact_type == FACT_TYPE_STATE
                    && (r.valid_until_ts == NULL_TIMESTAMP || r.valid_until_ts >= now_ts)
            })
            .collect()
    }

    /// Pattern 2: Forward scan since timestamp T.
    /// Returns all records with event_ts >= since_ts, sorted ascending.
    /// Uses binary search on the sorted records vec.
    pub fn events_since(&self, since_ts: i64) -> Vec<&TemporalRecord> {
        let start = self.records.partition_point(|r| r.event_ts < since_ts);
        self.records[start..].iter().collect()
    }

    /// Pattern 3: Facts matching a source_path_hash.
    /// Returns all records for that source file, sorted by event_ts.
    pub fn history_for_source(&self, source_path_hash: u64) -> Vec<&TemporalRecord> {
        self.records
            .iter()
            .filter(|r| r.source_path_hash == source_path_hash)
            .collect()
    }

    /// Returns true if temporal.log generation matches the provided generation.
    pub fn is_current(&self, generation: u64) -> bool {
        self.header.generation == generation
    }

    /// Returns the header for inspection.
    pub fn header(&self) -> &TemporalLogHeader {
        &self.header
    }

    /// Tier 2.5 search: dispatch on temporal query pattern.
    /// Returns empty if the temporal log generation does not match state_generation.
    pub fn tier2_5_search(
        &self,
        pattern: &TemporalQueryPattern,
        now_ts: i64,
        state_generation: u64,
    ) -> Vec<&TemporalRecord> {
        if !self.is_current(state_generation) {
            return vec![];
        }
        match pattern {
            TemporalQueryPattern::CurrentState => self.current_state_facts(now_ts),
            TemporalQueryPattern::SinceTimestamp(ts) => {
                let since = if *ts == i64::MIN { 0 } else { *ts };
                self.events_since(since)
            }
            TemporalQueryPattern::EventHistory => {
                let mut records: Vec<&TemporalRecord> = self.records.iter().collect();
                records.sort_by(|a, b| b.event_ts.cmp(&a.event_ts));
                records
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engram_core::temporal::{
        fnv1a_64, EVENT_KIND_CREATED, EVENT_KIND_EXPIRED, TEMPORAL_MAGIC, TEMPORAL_VERSION,
    };

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

    #[test]
    fn test_load_nonexistent_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let result = TemporalReader::load(tmp.path()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_current_state_excludes_expired() {
        let hash_a = fnv1a_64(b"a.md");
        let records = vec![
            // State fact, expired at ts=3000
            make_record(1000, FACT_TYPE_STATE, EVENT_KIND_CREATED, 3000, hash_a),
        ];
        let header = make_header(records.len() as u32, 1);
        let data = build_log(&header, &records);

        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("temporal.log"), &data).unwrap();

        let reader = TemporalReader::load(tmp.path()).unwrap().unwrap();

        // now_ts=5000 > valid_until_ts=3000 → excluded
        let current = reader.current_state_facts(5000);
        assert!(current.is_empty(), "expired state fact should be excluded");
    }

    #[test]
    fn test_current_state_includes_no_expiry() {
        let hash_a = fnv1a_64(b"a.md");
        let records = vec![
            // State fact, no expiry
            make_record(1000, FACT_TYPE_STATE, EVENT_KIND_CREATED, NULL_TIMESTAMP, hash_a),
        ];
        let header = make_header(records.len() as u32, 1);
        let data = build_log(&header, &records);

        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("temporal.log"), &data).unwrap();

        let reader = TemporalReader::load(tmp.path()).unwrap().unwrap();

        let current = reader.current_state_facts(5000);
        assert_eq!(current.len(), 1, "no-expiry state fact should be included");
    }

    #[test]
    fn test_events_since_binary_search() {
        let hash = fnv1a_64(b"x.md");
        let records = vec![
            make_record(1000, 0, EVENT_KIND_CREATED, NULL_TIMESTAMP, hash),
            make_record(2000, 0, EVENT_KIND_CREATED, NULL_TIMESTAMP, hash),
            make_record(3000, 0, EVENT_KIND_CREATED, NULL_TIMESTAMP, hash),
            make_record(4000, 0, EVENT_KIND_CREATED, NULL_TIMESTAMP, hash),
            make_record(5000, 0, EVENT_KIND_CREATED, NULL_TIMESTAMP, hash),
        ];
        let header = make_header(records.len() as u32, 1);
        let data = build_log(&header, &records);

        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("temporal.log"), &data).unwrap();

        let reader = TemporalReader::load(tmp.path()).unwrap().unwrap();

        let since = reader.events_since(3000);
        assert_eq!(since.len(), 3);
        assert_eq!(since[0].event_ts, 3000);
        assert_eq!(since[1].event_ts, 4000);
        assert_eq!(since[2].event_ts, 5000);
    }

    #[test]
    fn test_history_for_source() {
        let hash_a = fnv1a_64(b"a.md");
        let hash_b = fnv1a_64(b"b.md");
        let records = vec![
            make_record(1000, 0, EVENT_KIND_CREATED, NULL_TIMESTAMP, hash_a),
            make_record(2000, 0, EVENT_KIND_CREATED, NULL_TIMESTAMP, hash_b),
            make_record(3000, 0, EVENT_KIND_EXPIRED, 3000, hash_a),
        ];
        let header = make_header(records.len() as u32, 1);
        let data = build_log(&header, &records);

        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("temporal.log"), &data).unwrap();

        let reader = TemporalReader::load(tmp.path()).unwrap().unwrap();

        let history = reader.history_for_source(hash_a);
        assert_eq!(history.len(), 2);
        assert!(history.iter().all(|r| r.source_path_hash == hash_a));
    }

    #[test]
    fn test_stale_generation_detected() {
        let records = vec![];
        let header = make_header(0, 5);
        let data = build_log(&header, &records);

        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("temporal.log"), &data).unwrap();

        let reader = TemporalReader::load(tmp.path()).unwrap().unwrap();

        assert!(reader.is_current(5));
        assert!(!reader.is_current(6));
        assert!(!reader.is_current(4));
    }
}
