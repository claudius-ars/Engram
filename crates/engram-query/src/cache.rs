use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::result::QueryResult;

struct CacheEntry {
    result: QueryResult,
    inserted_at: Instant,
    generation: u64,
}

pub struct ExactCache {
    entries: HashMap<String, CacheEntry>,
    ttl_seconds: u64,
}

impl ExactCache {
    pub fn new(ttl_seconds: u64) -> Self {
        ExactCache {
            entries: HashMap::new(),
            ttl_seconds,
        }
    }

    pub fn get(
        &self,
        fingerprint: &str,
        current_generation: u64,
        dirty: bool,
    ) -> Option<&QueryResult> {
        if dirty {
            return None;
        }

        let entry = self.entries.get(fingerprint)?;

        if entry.generation != current_generation {
            return None;
        }

        if entry.inserted_at.elapsed() > Duration::from_secs(self.ttl_seconds) {
            return None;
        }

        Some(&entry.result)
    }

    pub fn insert(
        &mut self,
        fingerprint: String,
        result: QueryResult,
        generation: u64,
    ) {
        self.entries.insert(
            fingerprint,
            CacheEntry {
                result,
                inserted_at: Instant::now(),
                generation,
            },
        );
    }

    pub fn invalidate_all(&mut self) {
        self.entries.clear();
    }
}
