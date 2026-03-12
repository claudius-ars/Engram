use std::path::Path;

use engram_core::temporal::{
    fnv1a_64, TemporalLogHeader, TemporalRecord, EVENT_KIND_CREATED, EVENT_KIND_EXPIRED,
    EVENT_KIND_UPDATED, NULL_TIMESTAMP, TEMPORAL_MAGIC, TEMPORAL_VERSION,
};

use crate::manifest::ManifestEnvelope;

/// Write the temporal log to `.brv/index/temporal.log`.
///
/// For Phase 2, `previous_manifest` is always `None` (full rebuild).
/// When incremental compilation lands in Phase 3, pass the prior manifest
/// to emit `Updated` events for changed facts.
pub fn write_temporal_log(
    index_dir: &Path,
    manifest: &ManifestEnvelope,
    previous_manifest: Option<&ManifestEnvelope>,
    compiled_at_ts: i64,
    generation: u64,
) -> anyhow::Result<()> {
    let mut records = Vec::new();

    // Build a lookup of previous manifest entries by source_path for diffing
    let previous_entries: std::collections::HashMap<&str, &crate::manifest::ManifestEntry> =
        previous_manifest
            .map(|pm| {
                pm.entries
                    .iter()
                    .map(|e| (e.source_path.as_str(), e))
                    .collect()
            })
            .unwrap_or_default();

    for entry in &manifest.entries {
        let source_path_hash = fnv1a_64(entry.source_path.as_bytes());
        let content_hash = [0u8; 16]; // zeroed — fingerprints not available in Phase 2

        let created_at_ts = entry.created_at_ts;
        let valid_until_ts = entry.valid_until_ts;
        let updated_at_ts = entry.updated_at_ts;

        // Determine event_kind
        let prev = previous_entries.get(entry.source_path.as_str());

        let is_new = prev.is_none();
        let is_updated = prev
            .map(|p| p.updated_at_ts != entry.updated_at_ts)
            .unwrap_or(false);
        let is_expired = valid_until_ts != NULL_TIMESTAMP && valid_until_ts < compiled_at_ts;

        if is_updated {
            records.push(TemporalRecord {
                event_ts: updated_at_ts,
                valid_until_ts,
                created_at_ts,
                source_path_hash,
                content_hash,
                fact_type: entry.fact_type,
                event_kind: EVENT_KIND_UPDATED,
                _pad: [0u8; 14],
            });
        } else if is_new {
            records.push(TemporalRecord {
                event_ts: created_at_ts,
                valid_until_ts,
                created_at_ts,
                source_path_hash,
                content_hash,
                fact_type: entry.fact_type,
                event_kind: EVENT_KIND_CREATED,
                _pad: [0u8; 14],
            });
        } else {
            // Unchanged — re-emit Created
            records.push(TemporalRecord {
                event_ts: created_at_ts,
                valid_until_ts,
                created_at_ts,
                source_path_hash,
                content_hash,
                fact_type: entry.fact_type,
                event_kind: EVENT_KIND_CREATED,
                _pad: [0u8; 14],
            });
        }

        // A fact can be both Updated and Expired — emit Expired as a second record
        if is_expired {
            records.push(TemporalRecord {
                event_ts: valid_until_ts,
                valid_until_ts,
                created_at_ts,
                source_path_hash,
                content_hash,
                fact_type: entry.fact_type,
                event_kind: EVENT_KIND_EXPIRED,
                _pad: [0u8; 14],
            });
        }
    }

    // Sort by event_ts ascending
    records.sort_by_key(|r| r.event_ts);

    let header = TemporalLogHeader {
        magic: TEMPORAL_MAGIC,
        version: TEMPORAL_VERSION,
        record_count: records.len() as u32,
        compiled_at_ts,
        generation,
        _pad: [0u8; 32],
    };

    // Serialize to bytes
    let header_bytes: &[u8] = bytemuck::bytes_of(&header);
    let record_bytes: &[u8] = bytemuck::cast_slice(&records);

    let mut buf = Vec::with_capacity(64 + record_bytes.len());
    buf.extend_from_slice(header_bytes);
    buf.extend_from_slice(record_bytes);

    // Atomic write: write to .tmp, then rename
    let dest = index_dir.join("temporal.log");
    let tmp = index_dir.join("temporal.log.tmp");
    std::fs::write(&tmp, &buf)?;
    std::fs::rename(&tmp, &dest)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{ManifestEntry, ManifestEnvelope};

    fn make_entry(source_path: &str, fact_type: u8, created_at_ts: i64) -> ManifestEntry {
        ManifestEntry {
            id: source_path.to_string(),
            source_path: source_path.to_string(),
            fact_type,
            importance: 1.0,
            confidence: 1.0,
            recency: 1.0,
            created_at_ts,
            valid_until_ts: NULL_TIMESTAMP,
            updated_at_ts: NULL_TIMESTAMP,
        }
    }

    #[test]
    fn test_write_and_count() {
        let tmp = tempfile::tempdir().unwrap();
        let index_dir = tmp.path().join("index");
        std::fs::create_dir_all(&index_dir).unwrap();

        let manifest = ManifestEnvelope {
            version: 1,
            entries: vec![
                make_entry("a.md", 0, 1000),
                make_entry("b.md", 1, 2000),
                make_entry("c.md", 2, 3000),
            ],
        };

        write_temporal_log(&index_dir, &manifest, None, 5000, 1).unwrap();

        let data = std::fs::read(index_dir.join("temporal.log")).unwrap();
        let (header, records) =
            engram_core::temporal::parse_temporal_log(&data).unwrap();

        assert_eq!(header.record_count, 3);
        assert_eq!(records.len(), 3);
    }

    #[test]
    fn test_sorted_by_event_ts() {
        let tmp = tempfile::tempdir().unwrap();
        let index_dir = tmp.path().join("index");
        std::fs::create_dir_all(&index_dir).unwrap();

        let manifest = ManifestEnvelope {
            version: 1,
            entries: vec![
                make_entry("late.md", 0, 9000),
                make_entry("early.md", 0, 1000),
                make_entry("mid.md", 0, 5000),
            ],
        };

        write_temporal_log(&index_dir, &manifest, None, 10000, 1).unwrap();

        let data = std::fs::read(index_dir.join("temporal.log")).unwrap();
        let (_, records) =
            engram_core::temporal::parse_temporal_log(&data).unwrap();

        for i in 1..records.len() {
            assert!(
                records[i - 1].event_ts <= records[i].event_ts,
                "records should be sorted by event_ts: {} <= {}",
                records[i - 1].event_ts,
                records[i].event_ts
            );
        }
    }

    #[test]
    fn test_expired_fact_emits_expired_record() {
        let tmp = tempfile::tempdir().unwrap();
        let index_dir = tmp.path().join("index");
        std::fs::create_dir_all(&index_dir).unwrap();

        let mut entry = make_entry("expiring.md", 1, 1000);
        entry.valid_until_ts = 3000; // expires at ts=3000

        let manifest = ManifestEnvelope {
            version: 1,
            entries: vec![entry],
        };

        // compiled_at_ts=5000 > valid_until_ts=3000 → expired
        write_temporal_log(&index_dir, &manifest, None, 5000, 1).unwrap();

        let data = std::fs::read(index_dir.join("temporal.log")).unwrap();
        let (header, records) =
            engram_core::temporal::parse_temporal_log(&data).unwrap();

        // Should emit 2 records: Created + Expired
        assert_eq!(header.record_count, 2);
        assert_eq!(records.len(), 2);

        let created = records.iter().find(|r| r.event_kind == EVENT_KIND_CREATED);
        let expired = records.iter().find(|r| r.event_kind == EVENT_KIND_EXPIRED);

        assert!(created.is_some(), "should have a Created record");
        assert!(expired.is_some(), "should have an Expired record");
        assert_eq!(expired.unwrap().event_ts, 3000);
    }

    #[test]
    fn test_atomic_write() {
        let tmp = tempfile::tempdir().unwrap();
        let index_dir = tmp.path().join("index");
        std::fs::create_dir_all(&index_dir).unwrap();

        let manifest = ManifestEnvelope {
            version: 1,
            entries: vec![make_entry("a.md", 0, 1000)],
        };

        write_temporal_log(&index_dir, &manifest, None, 5000, 1).unwrap();

        // .tmp file should not exist after successful write
        assert!(
            !index_dir.join("temporal.log.tmp").exists(),
            ".tmp file should not exist after write"
        );
        // Final file should exist
        assert!(index_dir.join("temporal.log").exists());
    }
}
