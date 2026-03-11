pub mod curator;
pub mod indexer;
pub mod manifest;
pub mod parser;
pub mod state;
pub mod walker;

use std::path::Path;

use engram_bulwark::{AccessType, BulwarkHandle, PolicyDecision, PolicyRequest};

pub use indexer::{
    build_schema, IndexError, IndexStats, IndexWriter, CURRENT_SCHEMA_VERSION, NULL_TIMESTAMP,
};
pub use manifest::{ManifestEntry, ManifestEnvelope, ManifestError, ManifestStats, ManifestWriter, read_manifest, MANIFEST_VERSION};
pub use parser::{extract_frontmatter, parse_all, parse_file, ParseError, ParseResult};
pub use state::{read_state, write_state, fresh_state, IndexState, StateError};
pub use curator::{curate, CurateError, CurateOptions, CurateResult};
pub use walker::walk_context_tree;

pub struct CompileResult {
    pub parse_result: ParseResult,
    pub index_stats: Option<IndexStats>,
    pub index_error: Option<IndexError>,
    pub manifest_stats: Option<ManifestStats>,
    pub manifest_error: Option<ManifestError>,
    pub state: Option<IndexState>,
    pub state_error: Option<StateError>,
}

impl CompileResult {
    pub fn denied(reason: String) -> Self {
        CompileResult {
            parse_result: ParseResult::empty(),
            index_stats: None,
            index_error: Some(IndexError::PolicyDenied(reason)),
            manifest_stats: None,
            manifest_error: None,
            state: None,
            state_error: None,
        }
    }
}

/// Top-level function for the compile pipeline.
/// Discovers all .md files in root/.brv/context-tree/, parses them,
/// and optionally writes the Tantivy index, manifest, and state file.
pub fn compile_context_tree(root: &Path, write_index: bool, bulwark: &BulwarkHandle) -> CompileResult {
    // Policy check for write operations
    if write_index {
        let request = PolicyRequest {
            access_type: AccessType::Write,
            fact_id: None,
            agent_id: None,
            operation: "compile".to_string(),
        };

        if let PolicyDecision::Deny { reason } = bulwark.check(&request) {
            return CompileResult::denied(reason);
        }
    }

    let compile_start = std::time::Instant::now();

    let paths = match walk_context_tree(root) {
        Ok(p) => p,
        Err(e) => {
            return CompileResult {
                parse_result: ParseResult {
                    records: Vec::new(),
                    errors: vec![e],
                    warnings: Vec::new(),
                    file_count: 0,
                    error_count: 1,
                },
                index_stats: None,
                index_error: None,
                manifest_stats: None,
                manifest_error: None,
                state: None,
                state_error: None,
            };
        }
    };

    let parse_result = parse_all(paths);

    if !write_index {
        return CompileResult {
            parse_result,
            index_stats: None,
            index_error: None,
            manifest_stats: None,
            manifest_error: None,
            state: None,
            state_error: None,
        };
    }

    let index_dir = root.join(".brv").join("index");

    // Step 2: Read previous state for generation incrementing
    let previous_state = read_state(&index_dir).ok();

    // Step 3: Write Tantivy index
    let records_for_index = parse_result.records.clone();
    let index_writer = IndexWriter::new(root);

    let (index_stats, index_error) = match index_writer.write(records_for_index) {
        Ok(stats) => (Some(stats), None),
        Err(e) => {
            // Index write failed — skip manifest and state
            return CompileResult {
                parse_result,
                index_stats: None,
                index_error: Some(e),
                manifest_stats: None,
                manifest_error: None,
                state: None,
                state_error: None,
            };
        }
    };

    // Step 4: Write manifest
    let manifest_writer = ManifestWriter::new(root);
    let (manifest_stats, manifest_error) = match manifest_writer.write(&parse_result.records) {
        Ok(stats) => (Some(stats), None),
        Err(e) => (None, Some(e)),
    };

    // Step 5: Write state
    let duration_ms = compile_start.elapsed().as_millis() as u64;
    let mut new_state = fresh_state(parse_result.records.len() as u64, duration_ms);
    if let Some(prev) = previous_state {
        new_state.generation = prev.generation + 1;
    }

    // Ensure index dir exists for state file
    let _ = std::fs::create_dir_all(&index_dir);

    let state_error = match write_state(&index_dir, &new_state) {
        Ok(()) => None,
        Err(e) => Some(e),
    };

    CompileResult {
        parse_result,
        index_stats,
        index_error,
        manifest_stats,
        manifest_error,
        state: Some(new_state),
        state_error,
    }
}

// Re-export key types
pub use parser::ParseError as CompileError;

#[cfg(test)]
mod tests;
