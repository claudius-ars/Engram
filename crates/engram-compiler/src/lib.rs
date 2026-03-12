pub mod classification_cache;
pub mod classifier;
pub mod curator;
pub mod fingerprint;
pub mod indexer;
pub mod llm_classifier;
pub mod manifest;
pub mod parser;
pub mod state;
pub mod temporal_writer;
pub mod walker;
pub mod watcher;

use std::path::Path;

use engram_bulwark::{AccessType, BulwarkHandle, PolicyDecision, PolicyRequest};
use engram_core::CompileConfig;

pub use fingerprint::{
    compute_changes, detect_rename, load_fingerprints, make_fingerprint, save_fingerprints,
    ChangeSet, FingerprintEnvelope, FingerprintRecord, MtimeOnlyUpdate,
};

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

/// Top-level function for the compile pipeline (Phase 1 compatible).
/// Discovers all .md files in root/.brv/context-tree/, parses them,
/// and optionally writes the Tantivy index, manifest, and state file.
pub fn compile_context_tree(root: &Path, write_index: bool, bulwark: &BulwarkHandle) -> CompileResult {
    compile_context_tree_with_config(root, write_index, bulwark, &CompileConfig::default())
}

/// Top-level function for the compile pipeline with classification support.
pub fn compile_context_tree_with_config(
    root: &Path,
    write_index: bool,
    bulwark: &BulwarkHandle,
    compile_config: &CompileConfig,
) -> CompileResult {
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

    let mut parse_result = parse_all(paths);

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

    // Step 1b: Classification pipeline (only when --classify is set)
    if compile_config.classify {
        run_classification_pipeline(&mut parse_result, &index_dir, compile_config);
    }

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

    // Step 4b: Write temporal log (non-fatal — missing log degrades Tier 2.5 gracefully)
    if let Some(ref stats) = manifest_stats {
        let manifest_envelope = ManifestEnvelope {
            version: manifest::MANIFEST_VERSION,
            entries: parse_result
                .records
                .iter()
                .map(|r| ManifestEntry {
                    id: r.id.clone(),
                    source_path: r.source_path.to_string_lossy().to_string(),
                    fact_type: match r.fact_type {
                        engram_core::FactType::Durable => 0,
                        engram_core::FactType::State => 1,
                        engram_core::FactType::Event => 2,
                    },
                    importance: r.importance,
                    confidence: r.confidence,
                    recency: r.recency,
                    created_at_ts: r
                        .created_at
                        .map(|dt| dt.timestamp())
                        .unwrap_or(indexer::NULL_TIMESTAMP),
                    valid_until_ts: r
                        .valid_until
                        .map(|dt| dt.timestamp())
                        .unwrap_or(indexer::NULL_TIMESTAMP),
                    updated_at_ts: r
                        .updated_at
                        .map(|dt| dt.timestamp())
                        .unwrap_or(indexer::NULL_TIMESTAMP),
                })
                .collect(),
        };

        let generation = previous_state
            .as_ref()
            .map(|p| p.generation + 1)
            .unwrap_or(1);

        if let Err(e) = temporal_writer::write_temporal_log(
            &index_dir,
            &manifest_envelope,
            None,
            chrono::Utc::now().timestamp(),
            generation,
        ) {
            eprintln!("WARN: failed to write temporal log: {}", e);
        }
        let _ = stats; // suppress unused binding warning
    }

    // Step 5: Write state
    let duration_ms = compile_start.elapsed().as_millis() as u64;
    let mut new_state = fresh_state(parse_result.records.len() as u64, duration_ms);
    if let Some(prev) = previous_state {
        new_state.generation = prev.generation + 1;
    }

    // Ensure index dir exists for state file
    let _ = std::fs::create_dir_all(&index_dir);

    let state_error = write_state(&index_dir, &new_state).err();

    // Step 6: Save fingerprints for incremental compilation
    let generation = new_state.generation;
    let mut fp_env = FingerprintEnvelope::new();
    for record in &parse_result.records {
        let abs_path = if record.source_path.is_absolute() {
            record.source_path.clone()
        } else {
            root.join(&record.source_path)
        };
        if let Ok(fp) = make_fingerprint(&abs_path, root, 1, generation) {
            fp_env.entries.insert(fp.source_path.clone(), fp);
        }
    }
    save_fingerprints(&index_dir, &fp_env);

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

/// Perform incremental compilation: only reindex changed files.
/// Falls back to full rebuild if fingerprints are absent or empty.
pub fn compile_incremental(
    root: &Path,
    bulwark: &BulwarkHandle,
    compile_config: &CompileConfig,
) -> CompileResult {
    let index_dir = root.join(".brv").join("index");
    let fingerprints = load_fingerprints(&index_dir);

    // Fall back to full rebuild if no fingerprints
    if fingerprints.is_empty() {
        return compile_context_tree_with_config(root, true, bulwark, compile_config);
    }

    // Policy check
    let request = PolicyRequest {
        access_type: AccessType::Write,
        fact_id: None,
        agent_id: None,
        operation: "compile".to_string(),
    };
    if let PolicyDecision::Deny { reason } = bulwark.check(&request) {
        return CompileResult::denied(reason);
    }

    let compile_start = std::time::Instant::now();

    // Walk current files
    let current_files = match walk_context_tree(root) {
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

    // Compute changes
    let (changes, mtime_updates) = match compute_changes(&fingerprints, &current_files, root) {
        Ok(r) => r,
        Err(_) => {
            // Fall back to full rebuild on error
            return compile_context_tree_with_config(root, true, bulwark, compile_config);
        }
    };

    // Apply mtime-only updates
    let mut fp_env = FingerprintEnvelope {
        version: fingerprints.version,
        entries: fingerprints.entries.clone(),
    };
    for update in &mtime_updates {
        if let Some(fp) = fp_env.entries.get_mut(&update.rel_path) {
            fp.mtime_secs = update.new_mtime_secs;
            fp.mtime_nanos = update.new_mtime_nanos;
        }
    }

    // If no changes, just save mtime updates and return
    if changes.is_empty() {
        if !mtime_updates.is_empty() {
            save_fingerprints(&index_dir, &fp_env);
        }

        let previous_state = read_state(&index_dir).ok();
        let duration_ms = compile_start.elapsed().as_millis() as u64;
        let file_count = current_files.len();

        // Return result indicating no changes
        return CompileResult {
            parse_result: ParseResult {
                records: Vec::new(),
                errors: Vec::new(),
                warnings: Vec::new(),
                file_count,
                error_count: 0,
            },
            index_stats: Some(IndexStats {
                documents_written: 0,
                documents_skipped: 0,
                elapsed_ms: duration_ms,
            }),
            index_error: None,
            manifest_stats: None,
            manifest_error: None,
            state: previous_state,
            state_error: None,
        };
    }

    // Handle renames
    for added_path in &changes.added {
        let rel = added_path
            .strip_prefix(root)
            .unwrap_or(added_path)
            .to_string_lossy()
            .to_string();
        if let Ok(Some(_old_rel)) =
            detect_rename(&fingerprints, std::path::Path::new(&rel), &changes.deleted, root)
        {
            // Rename detected — the old entry will be deleted and new one added
        }
    }

    // Build deletions list — must use the same path format stored in Tantivy.
    // We use the index_source_path from fingerprint records, which is the exact
    // string that was stored in the Tantivy index at full-compile time.
    let mut deletions: Vec<String> = Vec::new();
    for del in &changes.deleted {
        if let Some(fp) = fingerprints.entries.get(del) {
            deletions.push(fp.index_source_path.clone());
        }
    }
    for mod_path in &changes.modified {
        let rel = mod_path
            .strip_prefix(root)
            .unwrap_or(mod_path)
            .to_string_lossy()
            .to_string();
        if let Some(fp) = fingerprints.entries.get(&rel) {
            deletions.push(fp.index_source_path.clone());
        } else {
            // Fallback: use the absolute path directly
            deletions.push(mod_path.to_string_lossy().to_string());
        }
    }

    // Parse added + modified files
    let files_to_parse: Vec<&std::path::PathBuf> = changes
        .added
        .iter()
        .chain(changes.modified.iter())
        .collect();

    let mut additions = Vec::new();
    let mut parse_errors = Vec::new();
    let mut parse_warnings = Vec::new();
    for path in &files_to_parse {
        match parser::parse_file(path) {
            Ok(record) => {
                parse_warnings.extend(record.warnings.clone());
                additions.push(record);
            }
            Err(e) => parse_errors.push(e),
        }
    }

    let parse_file_count = files_to_parse.len();
    let parse_error_count = parse_errors.len();

    // Open index and perform incremental update
    let (index, schema) = match indexer::open_index(root) {
        Ok(r) => r,
        Err(e) => {
            return CompileResult {
                parse_result: ParseResult {
                    records: additions,
                    errors: parse_errors,
                    warnings: parse_warnings,
                    file_count: parse_file_count,
                    error_count: parse_error_count,
                },
                index_stats: None,
                index_error: Some(e),
                manifest_stats: None,
                manifest_error: None,
                state: None,
                state_error: None,
            };
        }
    };

    let mut writer = match index.writer(50_000_000) {
        Ok(w) => w,
        Err(e) => {
            return CompileResult {
                parse_result: ParseResult {
                    records: additions,
                    errors: parse_errors,
                    warnings: parse_warnings,
                    file_count: parse_file_count,
                    error_count: parse_error_count,
                },
                index_stats: None,
                index_error: Some(IndexError::Tantivy(e)),
                manifest_stats: None,
                manifest_error: None,
                state: None,
                state_error: None,
            };
        }
    };

    let (index_stats, index_error) =
        match indexer::incremental_update(&schema, &mut writer, &deletions, &additions) {
            Ok(stats) => (Some(stats), None),
            Err(e) => (None, Some(e)),
        };

    // Update state
    let previous_state = read_state(&index_dir).ok();
    let duration_ms = compile_start.elapsed().as_millis() as u64;
    let mut new_state = fresh_state(current_files.len() as u64, duration_ms);
    if let Some(prev) = &previous_state {
        new_state.generation = prev.generation + 1;
    }
    let _ = std::fs::create_dir_all(&index_dir);
    let state_error = write_state(&index_dir, &new_state).err();

    // Update fingerprints
    let generation = new_state.generation;
    for del in &changes.deleted {
        fp_env.entries.remove(del);
    }
    for path in files_to_parse {
        if let Ok(fp) = make_fingerprint(path, root, 1, generation) {
            fp_env.entries.insert(fp.source_path.clone(), fp);
        }
    }
    save_fingerprints(&index_dir, &fp_env);

    // Write manifest (full re-parse to get all records for manifest)
    // For incremental, we rebuild the manifest from all current files
    let all_paths = walk_context_tree(root).unwrap_or_default();
    let all_parse = parser::parse_all(all_paths);
    let (manifest_stats, manifest_error) = {
        let manifest_writer = ManifestWriter::new(root);
        match manifest_writer.write(&all_parse.records) {
            Ok(stats) => (Some(stats), None),
            Err(e) => (None, Some(e)),
        }
    };

    CompileResult {
        parse_result: ParseResult {
            records: additions,
            errors: parse_errors,
            warnings: parse_warnings,
            file_count: parse_file_count,
            error_count: parse_error_count,
        },
        index_stats,
        index_error,
        manifest_stats,
        manifest_error,
        state: Some(new_state),
        state_error,
    }
}

/// Run the classification pipeline on unclassified facts.
/// Modifies records in-place: updates fact_type for facts that were classified.
fn run_classification_pipeline(
    parse_result: &mut ParseResult,
    index_dir: &std::path::Path,
    compile_config: &CompileConfig,
) {
    use classification_cache::{content_hash, load_classification_cache, save_classification_cache};
    use classifier::{rule_classify, RULE_CONFIDENCE_THRESHOLD};

    let mut cache = load_classification_cache(index_dir);
    let mut llm_queue: Vec<(usize, String, String)> = Vec::new(); // (index, hash, body)

    for (i, record) in parse_result.records.iter_mut().enumerate() {
        if record.fact_type_explicit {
            continue; // already has explicit factType — skip
        }

        let hash = content_hash(&record.body);

        // Check cache first
        if let Some(cached) = cache.get(&hash) {
            record.fact_type = cached.to_fact_type();
            continue;
        }

        // Run rule-based classifier
        let title = record.title.as_deref().unwrap_or("");
        let result = rule_classify(title, &record.body);

        if result.confidence >= RULE_CONFIDENCE_THRESHOLD {
            record.fact_type = result.to_fact_type();
            cache.insert(hash, result);
        } else {
            // Low confidence → enqueue for LLM
            llm_queue.push((i, hash, record.body.clone()));
        }
    }

    // LLM batch classification (Phase 2: uses stub)
    if !llm_queue.is_empty() {
        let api_key = std::env::var("ANTHROPIC_API_KEY").unwrap_or_default();
        if api_key.is_empty() {
            eprintln!(
                "WARN: --classify requires ANTHROPIC_API_KEY — LLM classification skipped ({} facts)",
                llm_queue.len()
            );
        } else {
            let facts_for_llm: Vec<(String, String)> = llm_queue
                .iter()
                .map(|(_, hash, body)| (hash.clone(), body.clone()))
                .collect();

            let llm_results = llm_classifier::classify_batch(
                &facts_for_llm,
                &api_key,
                compile_config.max_tokens_per_compile,
                &mut cache,
            );

            // Apply LLM results
            for (idx, hash, _) in &llm_queue {
                if let Some(result) = llm_results.get(hash) {
                    parse_result.records[*idx].fact_type = result.to_fact_type();
                }
            }
        }
    }

    // Save updated cache
    save_classification_cache(index_dir, &cache);
}

// Re-export key types
pub use parser::ParseError as CompileError;

#[cfg(test)]
mod tests;
