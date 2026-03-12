use std::path::{Path, PathBuf};

use engram_core::{FactRecord, FactType};
use serde::{Deserialize, Serialize};

use crate::indexer::NULL_TIMESTAMP;

pub const MANIFEST_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
pub struct ManifestEnvelope {
    pub version: u32,
    pub entries: Vec<ManifestEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub id: String,
    pub source_path: String,
    pub fact_type: u8,       // 0=durable, 1=state, 2=event
    pub importance: f64,
    pub confidence: f64,
    pub recency: f64,
    pub created_at_ts: i64,  // NULL_TIMESTAMP if none
    pub valid_until_ts: i64, // NULL_TIMESTAMP if none
    pub updated_at_ts: i64,  // NULL_TIMESTAMP if none
}

pub struct ManifestStats {
    pub entries_written: usize,
    pub size_bytes: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("bincode error: {0}")]
    Bincode(String),

    #[error("version mismatch: expected {expected}, got {got}")]
    VersionMismatch { expected: u32, got: u32 },
}

fn fact_type_to_u8(ft: &FactType) -> u8 {
    match ft {
        FactType::Durable => 0,
        FactType::State => 1,
        FactType::Event => 2,
    }
}

fn manifest_path(root: &Path) -> PathBuf {
    root.join(".brv").join("index").join("manifest.bin")
}

fn manifest_tmp_path(root: &Path) -> PathBuf {
    root.join(".brv").join("index").join("manifest.bin.tmp")
}

pub struct ManifestWriter {
    root: PathBuf,
}

impl ManifestWriter {
    pub fn new(root: &Path) -> Self {
        ManifestWriter {
            root: root.to_path_buf(),
        }
    }

    pub fn write(&self, records: &[FactRecord]) -> Result<ManifestStats, ManifestError> {
        let index_dir = self.root.join(".brv").join("index");
        std::fs::create_dir_all(&index_dir)?;

        let entries: Vec<ManifestEntry> = records
            .iter()
            .map(|r| ManifestEntry {
                id: r.id.clone(),
                source_path: r.source_path.to_string_lossy().to_string(),
                fact_type: fact_type_to_u8(&r.fact_type),
                importance: r.importance,
                confidence: r.confidence,
                recency: r.recency,
                created_at_ts: r
                    .created_at
                    .map(|dt| dt.timestamp())
                    .unwrap_or(NULL_TIMESTAMP),
                valid_until_ts: r
                    .valid_until
                    .map(|dt| dt.timestamp())
                    .unwrap_or(NULL_TIMESTAMP),
                updated_at_ts: r
                    .updated_at
                    .map(|dt| dt.timestamp())
                    .unwrap_or(NULL_TIMESTAMP),
            })
            .collect();

        let envelope = ManifestEnvelope {
            version: MANIFEST_VERSION,
            entries,
        };

        let bytes =
            bincode::serialize(&envelope).map_err(|e| ManifestError::Bincode(e.to_string()))?;

        let tmp = manifest_tmp_path(&self.root);
        let dest = manifest_path(&self.root);
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, &dest)?;

        let size_bytes = bytes.len() as u64;

        Ok(ManifestStats {
            entries_written: envelope.entries.len(),
            size_bytes,
        })
    }

    pub fn read(root: &Path) -> Result<Vec<ManifestEntry>, ManifestError> {
        let bytes = std::fs::read(manifest_path(root))?;
        let envelope: ManifestEnvelope =
            bincode::deserialize(&bytes).map_err(|e| ManifestError::Bincode(e.to_string()))?;
        if envelope.version != MANIFEST_VERSION {
            return Err(ManifestError::VersionMismatch {
                expected: MANIFEST_VERSION,
                got: envelope.version,
            });
        }
        Ok(envelope.entries)
    }
}

pub fn read_manifest(root: &Path) -> Result<Vec<ManifestEntry>, ManifestError> {
    ManifestWriter::read(root)
}
