use std::path::PathBuf;

use engram_bulwark::BulwarkHandle;
use engram_core::{load_workspace_config, WorkspaceConfig};
use engram_query::{ExactCache, FuzzyCache, QueryError, QueryOptions};

use crate::formatter::format_context_block;
use crate::{EnrichOptions, EnrichResult};

pub struct EngramPlugin {
    root: PathBuf,
    cache: ExactCache,
    fuzzy_cache: FuzzyCache,
    bulwark: BulwarkHandle,
    options: EnrichOptions,
    config: WorkspaceConfig,
}

impl EngramPlugin {
    /// Create a new plugin instance.
    /// root: the workspace root (directory containing .brv/)
    /// options: enrichment options
    pub fn new(root: PathBuf, options: EnrichOptions) -> Self {
        let config = load_workspace_config(&root.join(".brv"));
        EngramPlugin {
            cache: ExactCache::new(config.exact_cache_ttl_secs),
            fuzzy_cache: FuzzyCache::new(100),
            bulwark: BulwarkHandle::new_stub(),
            config,
            root,
            options,
        }
    }

    /// The before_prompt_build hook.
    /// Call this with the agent's current task string before
    /// constructing the prompt. Returns an EnrichResult containing
    /// the formatted context block and metadata.
    pub fn enrich(&mut self, task: &str) -> EnrichResult {
        let options = QueryOptions {
            max_results: self.options.max_facts,
            min_score: self.options.min_score,
            domain_tags: vec![],
            agent_id: "openclaw".to_string(),
        };

        let query_result = engram_query::query(
            &self.root,
            task,
            options,
            &mut self.cache,
            &mut self.fuzzy_cache,
            &self.bulwark,
            &self.config,
        );

        match query_result {
            Ok(result) => {
                let fact_count = result.hits.len();
                let cache_tier = result.meta.cache_tier;
                let stale = result.meta.stale;
                let context_block = format_context_block(&result, &self.options);
                EnrichResult {
                    context_block,
                    from_index: true,
                    fact_count,
                    cache_tier: Some(cache_tier),
                    stale,
                }
            }
            Err(QueryError::IndexNotFound) => {
                let context_block = self
                    .options
                    .fallback_message
                    .clone()
                    .unwrap_or_default();
                EnrichResult {
                    context_block,
                    from_index: false,
                    fact_count: 0,
                    cache_tier: None,
                    stale: false,
                }
            }
            Err(_) => {
                // Unexpected error — degrade gracefully rather than panic.
                // Phase 4 adds structured error logging here.
                EnrichResult {
                    context_block: String::new(),
                    from_index: false,
                    fact_count: 0,
                    cache_tier: None,
                    stale: true,
                }
            }
        }
    }
}
