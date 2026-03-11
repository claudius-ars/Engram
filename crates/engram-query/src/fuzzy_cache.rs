use std::collections::HashSet;
use std::time::{Duration, Instant};

use crate::result::QueryResult;

struct FuzzyCacheEntry {
    #[allow(dead_code)]
    original_query: String,
    tokens: HashSet<String>,
    result: QueryResult,
    inserted_at: Instant,
    generation: u64,
}

pub struct FuzzyCache {
    entries: Vec<FuzzyCacheEntry>,
    max_entries: usize,
}

impl FuzzyCache {
    pub fn new(max_entries: usize) -> Self {
        FuzzyCache {
            entries: Vec::new(),
            max_entries,
        }
    }

    /// Tokenize a query string into a normalized token set.
    pub fn tokenize(query: &str) -> HashSet<String> {
        query
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| !t.is_empty())
            .map(|t| t.to_string())
            .collect()
    }

    /// Compute Jaccard similarity between two token sets.
    pub fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
        if a.is_empty() && b.is_empty() {
            return 1.0;
        }
        let intersection = a.intersection(b).count();
        let union = a.union(b).count();
        if union == 0 {
            return 0.0;
        }
        intersection as f64 / union as f64
    }

    /// Find the best matching cached entry above the threshold.
    pub fn get(
        &self,
        query_tokens: &HashSet<String>,
        threshold: f64,
        current_generation: u64,
        dirty: bool,
        ttl_seconds: u64,
    ) -> Option<&QueryResult> {
        if dirty {
            return None;
        }

        let ttl = Duration::from_secs(ttl_seconds);
        let mut best: Option<(f64, Instant, &QueryResult)> = None;

        for entry in &self.entries {
            if entry.generation != current_generation {
                continue;
            }
            if entry.inserted_at.elapsed() > ttl {
                continue;
            }

            let sim = Self::jaccard(&entry.tokens, query_tokens);
            if sim < threshold {
                continue;
            }

            match best {
                None => best = Some((sim, entry.inserted_at, &entry.result)),
                Some((best_sim, best_time, _)) => {
                    if sim > best_sim || (sim == best_sim && entry.inserted_at > best_time) {
                        best = Some((sim, entry.inserted_at, &entry.result));
                    }
                }
            }
        }

        best.map(|(_, _, result)| result)
    }

    /// Insert a new entry. If max_entries is reached, evict the oldest entry.
    pub fn insert(
        &mut self,
        query: String,
        result: QueryResult,
        generation: u64,
    ) {
        if self.entries.len() >= self.max_entries {
            // Evict the oldest entry (smallest inserted_at)
            if let Some(oldest_idx) = self
                .entries
                .iter()
                .enumerate()
                .min_by_key(|(_, e)| e.inserted_at)
                .map(|(i, _)| i)
            {
                self.entries.swap_remove(oldest_idx);
            }
        }

        let tokens = Self::tokenize(&query);
        self.entries.push(FuzzyCacheEntry {
            original_query: query,
            tokens,
            result,
            inserted_at: Instant::now(),
            generation,
        });
    }

    pub fn invalidate_all(&mut self) {
        self.entries.clear();
    }
}
