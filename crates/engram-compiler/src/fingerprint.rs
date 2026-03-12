use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const CURRENT_VERSION: u32 = 1;

/// One entry per .md file successfully indexed.
/// Relative paths only — absolute paths break when workspace moves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FingerprintRecord {
    /// Path relative to workspace root
    pub source_path: String,
    /// Absolute path as stored in the Tantivy index (used for delete_term matching)
    pub index_source_path: String,
    /// mtime seconds since Unix epoch
    pub mtime_secs: i64,
    pub mtime_nanos: u32,
    /// BLAKE3 hash of file content at last index time
    pub content_hash: [u8; 32],
    /// Number of facts extracted from this file
    pub fact_count: u32,
    /// state.generation at the time this file was indexed
    pub indexed_at_generation: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FingerprintEnvelope {
    pub version: u32,
    pub entries: HashMap<String, FingerprintRecord>,
}

impl FingerprintEnvelope {
    pub fn new() -> Self {
        FingerprintEnvelope {
            version: CURRENT_VERSION,
            entries: HashMap::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for FingerprintEnvelope {
    fn default() -> Self {
        Self::new()
    }
}

pub struct ChangeSet {
    pub added: Vec<PathBuf>,
    pub modified: Vec<PathBuf>,
    /// Relative paths of deleted files
    pub deleted: Vec<String>,
}

impl ChangeSet {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.modified.is_empty() && self.deleted.is_empty()
    }
}

pub struct MtimeOnlyUpdate {
    pub rel_path: String,
    pub new_mtime_secs: i64,
    pub new_mtime_nanos: u32,
}

fn fingerprint_path(index_dir: &Path) -> PathBuf {
    index_dir.join("fingerprints.bin")
}

fn fingerprint_tmp_path(index_dir: &Path) -> PathBuf {
    index_dir.join("fingerprints.bin.tmp")
}

/// Load fingerprint envelope from disk. Infallible: returns empty envelope on any failure.
pub fn load_fingerprints(index_dir: &Path) -> FingerprintEnvelope {
    let path = fingerprint_path(index_dir);

    if !path.exists() {
        return FingerprintEnvelope::new();
    }

    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("WARN: failed to read {}: {}", path.display(), e);
            return FingerprintEnvelope::new();
        }
    };

    match bincode::deserialize::<FingerprintEnvelope>(&bytes) {
        Ok(env) => {
            if env.version != CURRENT_VERSION {
                eprintln!(
                    "WARN: fingerprint version mismatch (expected {}, got {}), starting fresh",
                    CURRENT_VERSION, env.version
                );
                return FingerprintEnvelope::new();
            }
            env
        }
        Err(e) => {
            eprintln!(
                "WARN: failed to decode {}: {}",
                path.display(),
                e
            );
            FingerprintEnvelope::new()
        }
    }
}

/// Save fingerprint envelope to disk using atomic write (tmp + rename).
pub fn save_fingerprints(index_dir: &Path, envelope: &FingerprintEnvelope) {
    let _ = std::fs::create_dir_all(index_dir);
    let tmp = fingerprint_tmp_path(index_dir);
    let dest = fingerprint_path(index_dir);

    match bincode::serialize(envelope) {
        Ok(bytes) => {
            if let Err(e) = std::fs::write(&tmp, &bytes) {
                eprintln!("WARN: failed to write {}: {}", tmp.display(), e);
                return;
            }
            if let Err(e) = std::fs::rename(&tmp, &dest) {
                eprintln!("WARN: failed to rename {} → {}: {}", tmp.display(), dest.display(), e);
            }
        }
        Err(e) => {
            eprintln!("WARN: failed to serialize fingerprints: {}", e);
        }
    }
}

/// Compute the BLAKE3 hash of a file's content.
pub fn hash_file(path: &Path) -> anyhow::Result<[u8; 32]> {
    let content = std::fs::read(path)?;
    Ok(*blake3::hash(&content).as_bytes())
}

/// Get (mtime_secs, mtime_nanos) from file metadata.
fn get_mtime(path: &Path) -> anyhow::Result<(i64, u32)> {
    use std::time::UNIX_EPOCH;
    let metadata = std::fs::metadata(path)?;
    let mtime = metadata.modified()?;
    let duration = mtime.duration_since(UNIX_EPOCH).unwrap_or_default();
    Ok((duration.as_secs() as i64, duration.subsec_nanos()))
}

/// Given current fingerprints and current filesystem state, compute changes.
///
/// Algorithm:
/// 1. For each file on disk:
///    a. If not in fingerprints → added
///    b. If mtime unchanged → skip (no hash needed)
///    c. If mtime changed → hash file
///       - hash unchanged → mtime-only update (no reindex)
///       - hash changed   → modified (reindex required)
/// 2. Anything in fingerprints not on disk → deleted
pub fn compute_changes(
    fingerprints: &FingerprintEnvelope,
    current_files: &[PathBuf],
    workspace_root: &Path,
) -> anyhow::Result<(ChangeSet, Vec<MtimeOnlyUpdate>)> {
    let mut added = Vec::new();
    let mut modified = Vec::new();
    let mut mtime_updates = Vec::new();
    let mut seen_rel_paths = std::collections::HashSet::new();

    for file in current_files {
        let rel_path = file
            .strip_prefix(workspace_root)
            .unwrap_or(file)
            .to_string_lossy()
            .to_string();
        seen_rel_paths.insert(rel_path.clone());

        match fingerprints.entries.get(&rel_path) {
            None => {
                added.push(file.clone());
            }
            Some(fp) => {
                let (mtime_secs, mtime_nanos) = get_mtime(file)?;
                if mtime_secs == fp.mtime_secs && mtime_nanos == fp.mtime_nanos {
                    // Unchanged — skip
                    continue;
                }
                // mtime changed — check content hash
                let hash = hash_file(file)?;
                if hash == fp.content_hash {
                    // Content same, only mtime changed
                    mtime_updates.push(MtimeOnlyUpdate {
                        rel_path,
                        new_mtime_secs: mtime_secs,
                        new_mtime_nanos: mtime_nanos,
                    });
                } else {
                    modified.push(file.clone());
                }
            }
        }
    }

    // Anything in fingerprints not on disk → deleted
    let deleted: Vec<String> = fingerprints
        .entries
        .keys()
        .filter(|k| !seen_rel_paths.contains(*k))
        .cloned()
        .collect();

    Ok((
        ChangeSet {
            added,
            modified,
            deleted,
        },
        mtime_updates,
    ))
}

/// Returns the relative path of the old file if `added_path` is a rename of a deleted file.
/// Detects rename by matching BLAKE3 content hash against deleted fingerprint entries.
pub fn detect_rename(
    fingerprints: &FingerprintEnvelope,
    added_path: &Path,
    deleted_rel_paths: &[String],
    workspace_root: &Path,
) -> anyhow::Result<Option<String>> {
    if deleted_rel_paths.is_empty() {
        return Ok(None);
    }

    let new_hash = hash_file(&workspace_root.join(added_path))?;

    for del_path in deleted_rel_paths {
        if let Some(fp) = fingerprints.entries.get(del_path) {
            if fp.content_hash == new_hash {
                return Ok(Some(del_path.clone()));
            }
        }
    }

    Ok(None)
}

/// Build a FingerprintRecord for a file that was just indexed.
pub fn make_fingerprint(
    path: &Path,
    workspace_root: &Path,
    fact_count: u32,
    generation: u64,
) -> anyhow::Result<FingerprintRecord> {
    let rel_path = path
        .strip_prefix(workspace_root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string();
    let (mtime_secs, mtime_nanos) = get_mtime(path)?;
    let content_hash = hash_file(path)?;

    let index_source_path = path.to_string_lossy().to_string();

    Ok(FingerprintRecord {
        source_path: rel_path,
        index_source_path,
        mtime_secs,
        mtime_nanos,
        content_hash,
        fact_count,
        indexed_at_generation: generation,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fingerprint_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let mut env = FingerprintEnvelope::new();
        env.entries.insert(
            "a.md".to_string(),
            FingerprintRecord {
                source_path: "a.md".to_string(),
                index_source_path: "/ws/a.md".to_string(),
                mtime_secs: 1000,
                mtime_nanos: 500,
                content_hash: [1u8; 32],
                fact_count: 2,
                indexed_at_generation: 1,
            },
        );
        env.entries.insert(
            "b.md".to_string(),
            FingerprintRecord {
                source_path: "b.md".to_string(),
                index_source_path: "/ws/b.md".to_string(),
                mtime_secs: 2000,
                mtime_nanos: 0,
                content_hash: [2u8; 32],
                fact_count: 1,
                indexed_at_generation: 1,
            },
        );
        env.entries.insert(
            "c.md".to_string(),
            FingerprintRecord {
                source_path: "c.md".to_string(),
                index_source_path: "/ws/c.md".to_string(),
                mtime_secs: 3000,
                mtime_nanos: 100,
                content_hash: [3u8; 32],
                fact_count: 3,
                indexed_at_generation: 2,
            },
        );

        save_fingerprints(tmp.path(), &env);
        let loaded = load_fingerprints(tmp.path());

        assert_eq!(loaded.entries.len(), 3);
        assert_eq!(loaded.entries.get("a.md"), env.entries.get("a.md"));
        assert_eq!(loaded.entries.get("b.md"), env.entries.get("b.md"));
        assert_eq!(loaded.entries.get("c.md"), env.entries.get("c.md"));
    }

    #[test]
    fn test_fingerprint_version_mismatch_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let bad_env = FingerprintEnvelope {
            version: 99,
            entries: {
                let mut m = HashMap::new();
                m.insert(
                    "x.md".to_string(),
                    FingerprintRecord {
                        source_path: "x.md".to_string(),
                        index_source_path: "/ws/x.md".to_string(),
                        mtime_secs: 100,
                        mtime_nanos: 0,
                        content_hash: [0u8; 32],
                        fact_count: 1,
                        indexed_at_generation: 1,
                    },
                );
                m
            },
        };
        let bytes = bincode::serialize(&bad_env).unwrap();
        std::fs::write(tmp.path().join("fingerprints.bin"), bytes).unwrap();

        let loaded = load_fingerprints(tmp.path());
        assert!(loaded.entries.is_empty());
    }

    #[test]
    fn test_compute_changes_added() {
        let tmp = tempfile::tempdir().unwrap();
        let file_path = tmp.path().join("new.md");
        std::fs::write(&file_path, "hello").unwrap();

        let env = FingerprintEnvelope::new();
        let (changes, mtime_updates) =
            compute_changes(&env, std::slice::from_ref(&file_path), tmp.path()).unwrap();

        assert_eq!(changes.added.len(), 1);
        assert!(changes.modified.is_empty());
        assert!(changes.deleted.is_empty());
        assert!(mtime_updates.is_empty());
    }

    #[test]
    fn test_compute_changes_modified() {
        let tmp = tempfile::tempdir().unwrap();
        let file_path = tmp.path().join("fact.md");
        std::fs::write(&file_path, "original content").unwrap();

        let hash = hash_file(&file_path).unwrap();
        let rel = "fact.md".to_string();

        let mut env = FingerprintEnvelope::new();
        env.entries.insert(
            rel.clone(),
            FingerprintRecord {
                source_path: rel,
                index_source_path: "/ws/fact.md".to_string(),
                mtime_secs: 1000, // old mtime
                mtime_nanos: 0,
                content_hash: [0u8; 32], // different hash
                fact_count: 1,
                indexed_at_generation: 1,
            },
        );

        // File has different mtime (it was just created) and different hash
        let (changes, _) =
            compute_changes(&env, std::slice::from_ref(&file_path), tmp.path()).unwrap();

        assert!(changes.added.is_empty());
        assert_eq!(changes.modified.len(), 1);
        assert!(changes.deleted.is_empty());
        // Verify hash is different from the stored one
        assert_ne!(hash, [0u8; 32]);
    }

    #[test]
    fn test_compute_changes_mtime_only() {
        let tmp = tempfile::tempdir().unwrap();
        let file_path = tmp.path().join("fact.md");
        std::fs::write(&file_path, "same content").unwrap();

        let hash = hash_file(&file_path).unwrap();
        let rel = "fact.md".to_string();

        let mut env = FingerprintEnvelope::new();
        env.entries.insert(
            rel.clone(),
            FingerprintRecord {
                source_path: rel,
                index_source_path: "/ws/fact.md".to_string(),
                mtime_secs: 1000, // old mtime — triggers hash check
                mtime_nanos: 0,
                content_hash: hash, // same hash — should be mtime-only
                fact_count: 1,
                indexed_at_generation: 1,
            },
        );

        let (changes, mtime_updates) =
            compute_changes(&env, &[file_path], tmp.path()).unwrap();

        assert!(changes.added.is_empty());
        assert!(changes.modified.is_empty());
        assert!(changes.deleted.is_empty());
        assert_eq!(mtime_updates.len(), 1);
        assert_eq!(mtime_updates[0].rel_path, "fact.md");
    }

    #[test]
    fn test_compute_changes_deleted() {
        let tmp = tempfile::tempdir().unwrap();
        let rel = "gone.md".to_string();

        let mut env = FingerprintEnvelope::new();
        env.entries.insert(
            rel.clone(),
            FingerprintRecord {
                source_path: rel,
                index_source_path: "/ws/gone.md".to_string(),
                mtime_secs: 1000,
                mtime_nanos: 0,
                content_hash: [0u8; 32],
                fact_count: 1,
                indexed_at_generation: 1,
            },
        );

        // No files on disk
        let (changes, mtime_updates) =
            compute_changes(&env, &[], tmp.path()).unwrap();

        assert!(changes.added.is_empty());
        assert!(changes.modified.is_empty());
        assert_eq!(changes.deleted.len(), 1);
        assert_eq!(changes.deleted[0], "gone.md");
        assert!(mtime_updates.is_empty());
    }

    #[test]
    fn test_compute_changes_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let file_path = tmp.path().join("stable.md");
        std::fs::write(&file_path, "stable content").unwrap();

        let hash = hash_file(&file_path).unwrap();
        let (mtime_secs, mtime_nanos) = {
            use std::time::UNIX_EPOCH;
            let md = std::fs::metadata(&file_path).unwrap();
            let mtime = md.modified().unwrap();
            let dur = mtime.duration_since(UNIX_EPOCH).unwrap();
            (dur.as_secs() as i64, dur.subsec_nanos())
        };
        let rel = "stable.md".to_string();

        let mut env = FingerprintEnvelope::new();
        env.entries.insert(
            rel.clone(),
            FingerprintRecord {
                source_path: rel,
                index_source_path: "/ws/stable.md".to_string(),
                mtime_secs,
                mtime_nanos,
                content_hash: hash,
                fact_count: 1,
                indexed_at_generation: 1,
            },
        );

        let (changes, mtime_updates) =
            compute_changes(&env, &[file_path], tmp.path()).unwrap();

        assert!(changes.is_empty());
        assert!(mtime_updates.is_empty());
    }

    #[test]
    fn test_detect_rename() {
        let tmp = tempfile::tempdir().unwrap();
        let new_file = tmp.path().join("new_name.md");
        std::fs::write(&new_file, "same content for rename").unwrap();
        let hash = hash_file(&new_file).unwrap();

        let mut env = FingerprintEnvelope::new();
        env.entries.insert(
            "old_name.md".to_string(),
            FingerprintRecord {
                source_path: "old_name.md".to_string(),
                index_source_path: "/ws/old_name.md".to_string(),
                mtime_secs: 1000,
                mtime_nanos: 0,
                content_hash: hash, // same content
                fact_count: 1,
                indexed_at_generation: 1,
            },
        );

        let result = detect_rename(
            &env,
            Path::new("new_name.md"),
            &["old_name.md".to_string()],
            tmp.path(),
        )
        .unwrap();

        assert_eq!(result, Some("old_name.md".to_string()));
    }

    #[test]
    fn test_detect_rename_independent_add() {
        let tmp = tempfile::tempdir().unwrap();
        let new_file = tmp.path().join("brand_new.md");
        std::fs::write(&new_file, "totally different content").unwrap();

        let mut env = FingerprintEnvelope::new();
        env.entries.insert(
            "deleted.md".to_string(),
            FingerprintRecord {
                source_path: "deleted.md".to_string(),
                index_source_path: "/ws/deleted.md".to_string(),
                mtime_secs: 1000,
                mtime_nanos: 0,
                content_hash: [99u8; 32], // different content
                fact_count: 1,
                indexed_at_generation: 1,
            },
        );

        let result = detect_rename(
            &env,
            Path::new("brand_new.md"),
            &["deleted.md".to_string()],
            tmp.path(),
        )
        .unwrap();

        assert_eq!(result, None);
    }
}
