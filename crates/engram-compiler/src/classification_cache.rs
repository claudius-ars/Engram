use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::classifier::ClassificationResult;

const CACHE_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
pub struct ClassificationCache {
    pub version: u32,
    pub entries: HashMap<String, ClassificationResult>,
}

impl Default for ClassificationCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ClassificationCache {
    pub fn new() -> Self {
        ClassificationCache {
            version: CACHE_VERSION,
            entries: HashMap::new(),
        }
    }

    /// Look up a cached classification by content hash.
    pub fn get(&self, content_hash: &str) -> Option<&ClassificationResult> {
        self.entries.get(content_hash)
    }

    /// Insert a classification result into the cache.
    pub fn insert(&mut self, content_hash: String, result: ClassificationResult) {
        self.entries.insert(content_hash, result);
    }
}

/// Load classification cache from disk. Infallible: returns empty cache on any failure.
pub fn load_classification_cache(index_dir: &Path) -> ClassificationCache {
    let path = index_dir.join("classification_cache.json");

    if !path.exists() {
        return ClassificationCache::new();
    }

    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "WARN: failed to read {}: {}",
                path.display(),
                e
            );
            return ClassificationCache::new();
        }
    };

    match serde_json::from_str::<ClassificationCache>(&content) {
        Ok(cache) => {
            if cache.version != CACHE_VERSION {
                eprintln!(
                    "WARN: classification cache version mismatch (expected {}, got {}), starting fresh",
                    CACHE_VERSION, cache.version
                );
                return ClassificationCache::new();
            }
            cache
        }
        Err(e) => {
            eprintln!(
                "WARN: failed to parse {}: {}",
                path.display(),
                e
            );
            ClassificationCache::new()
        }
    }
}

/// Save classification cache to disk. Non-fatal: logs WARN on failure.
pub fn save_classification_cache(index_dir: &Path, cache: &ClassificationCache) {
    let path = index_dir.join("classification_cache.json");
    let _ = std::fs::create_dir_all(index_dir);

    match serde_json::to_string_pretty(cache) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                eprintln!("WARN: failed to write {}: {}", path.display(), e);
            }
        }
        Err(e) => {
            eprintln!("WARN: failed to serialize classification cache: {}", e);
        }
    }
}

/// Compute a hex-encoded MD5 hash of the fact body for cache keying.
pub fn content_hash(body: &str) -> String {
    format!("{:x}", md5::compute(body.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classifier::ClassificationMethod;

    #[test]
    fn test_cache_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cache = ClassificationCache::new();
        cache.insert(
            "abc123".to_string(),
            ClassificationResult {
                fact_type: "state".to_string(),
                confidence: 0.92,
                method: ClassificationMethod::Rules,
                classified_at: Some("2024-03-15T10:30:00Z".to_string()),
            },
        );

        save_classification_cache(tmp.path(), &cache);

        let loaded = load_classification_cache(tmp.path());
        assert_eq!(loaded.entries.len(), 1);
        let entry = loaded.get("abc123").unwrap();
        assert_eq!(entry.fact_type, "state");
        assert!((entry.confidence - 0.92).abs() < f32::EPSILON);
        assert_eq!(entry.method, ClassificationMethod::Rules);
    }

    #[test]
    fn test_cache_absent_file_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = load_classification_cache(tmp.path());
        assert!(cache.entries.is_empty());
    }

    #[test]
    fn test_cache_malformed_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("classification_cache.json"),
            "not valid json {{{}}}",
        )
        .unwrap();

        let cache = load_classification_cache(tmp.path());
        assert!(cache.entries.is_empty());
    }
}
