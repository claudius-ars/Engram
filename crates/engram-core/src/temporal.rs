use bytemuck::{Pod, Zeroable};

/// Magic bytes identifying a temporal log file.
pub const TEMPORAL_MAGIC: [u8; 8] = *b"ENGRTLOG";

/// Current temporal log format version.
pub const TEMPORAL_VERSION: u32 = 1;

/// Sentinel value for "no expiry" / "no timestamp".
pub const NULL_TIMESTAMP: i64 = i64::MIN;

/// Event kind discriminants.
pub const EVENT_KIND_CREATED: u8 = 0;
pub const EVENT_KIND_UPDATED: u8 = 1;
pub const EVENT_KIND_EXPIRED: u8 = 2;

/// Fact type discriminants (mirrors indexer convention).
pub const FACT_TYPE_DURABLE: u8 = 0;
pub const FACT_TYPE_STATE: u8 = 1;
pub const FACT_TYPE_EVENT: u8 = 2;

/// File header — exactly 64 bytes, at byte offset 0.
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub struct TemporalLogHeader {
    pub magic: [u8; 8],
    pub version: u32,
    pub record_count: u32,
    pub compiled_at_ts: i64,
    pub generation: u64,
    pub _pad: [u8; 32],
}

const _: () = assert!(std::mem::size_of::<TemporalLogHeader>() == 64);

/// Event record — exactly 64 bytes, repeated `record_count` times.
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub struct TemporalRecord {
    pub event_ts: i64,
    pub valid_until_ts: i64,
    pub created_at_ts: i64,
    pub source_path_hash: u64,
    pub content_hash: [u8; 16],
    pub fact_type: u8,
    pub event_kind: u8,
    pub _pad: [u8; 14],
}

const _: () = assert!(std::mem::size_of::<TemporalRecord>() == 64);

/// FNV-1a 64-bit hash. Used for source_path hashing.
pub fn fnv1a_64(data: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 14695981039346656037;
    const PRIME: u64 = 1099511628211;
    let mut hash = OFFSET_BASIS;
    for byte in data {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

/// Parse raw bytes into a header and record slice.
/// Returns an error if the data is too short or has invalid magic/version.
pub fn parse_temporal_log(data: &[u8]) -> anyhow::Result<(TemporalLogHeader, &[TemporalRecord])> {
    if data.len() < 64 {
        anyhow::bail!("temporal.log too short for header ({} bytes)", data.len());
    }

    let header: TemporalLogHeader = *bytemuck::from_bytes(&data[..64]);

    if header.magic != TEMPORAL_MAGIC {
        anyhow::bail!(
            "invalid temporal.log magic: {:?}",
            &header.magic
        );
    }

    if header.version != TEMPORAL_VERSION {
        anyhow::bail!(
            "temporal.log version mismatch: expected {}, got {}",
            TEMPORAL_VERSION,
            header.version
        );
    }

    let record_bytes = &data[64..];
    let expected_len = header.record_count as usize * 64;
    if record_bytes.len() < expected_len {
        anyhow::bail!(
            "temporal.log truncated: expected {} record bytes, got {}",
            expected_len,
            record_bytes.len()
        );
    }

    let records: &[TemporalRecord] =
        bytemuck::cast_slice(&record_bytes[..expected_len]);

    Ok((header, records))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_sizes() {
        assert_eq!(std::mem::size_of::<TemporalLogHeader>(), 64);
        assert_eq!(std::mem::size_of::<TemporalRecord>(), 64);
    }

    #[test]
    fn test_fnv_hash_stability() {
        let h1 = fnv1a_64(b"context-tree/k8s.md");
        let h2 = fnv1a_64(b"context-tree/k8s.md");
        assert_eq!(h1, h2, "same input must produce same hash");

        let h3 = fnv1a_64(b"context-tree/redis.md");
        assert_ne!(h1, h3, "different inputs should produce different hashes");
    }
}
