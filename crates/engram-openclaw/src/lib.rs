pub mod formatter;
pub mod plugin;

use std::path::Path;

pub use plugin::EngramPlugin;

#[derive(Debug, Clone)]
pub struct EnrichOptions {
    pub max_facts: usize,
    pub min_score: f64,
    pub include_metadata: bool,
    pub fallback_message: Option<String>,
}

impl Default for EnrichOptions {
    fn default() -> Self {
        EnrichOptions {
            max_facts: 5,
            min_score: 0.1,
            include_metadata: false,
            fallback_message: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct EnrichResult {
    pub context_block: String,
    pub from_index: bool,
    pub fact_count: usize,
    pub cache_tier: Option<u8>,
    pub stale: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum EnrichError {
    #[error("query failed: {0}")]
    Query(#[from] engram_query::QueryError),

    #[error("policy denied: {0}")]
    PolicyDenied(String),
}

/// One-shot enrichment. Creates a temporary plugin instance.
/// For repeated calls, use EngramPlugin directly — it reuses
/// the cache across calls.
pub fn enrich_once(root: &Path, task: &str, options: EnrichOptions) -> EnrichResult {
    let mut plugin = EngramPlugin::new(root.to_path_buf(), options);
    plugin.enrich(task)
}

#[cfg(test)]
mod tests;
