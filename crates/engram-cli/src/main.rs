use clap::{Parser, Subcommand};
use engram_bulwark::BulwarkHandle;
use engram_core::{load_workspace_config, CausalValidationWarning};
use engram_query::{ExactCache, FuzzyCache, QueryError, QueryOptions};

#[derive(Parser)]
#[command(name = "engram", version, about = "Engram memory compiler and query engine")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Compile memory files into the index
    Compile {
        /// Watch for file changes and recompile automatically
        #[arg(long)]
        watch: bool,
        /// Run classification pipeline on unclassified facts
        #[arg(long)]
        classify: bool,
        /// Use incremental compilation (reindex only changed files)
        #[arg(long)]
        incremental: bool,
        /// Agent ID for Bulwark policy evaluation (default: "cli")
        #[arg(long, default_value = "cli")]
        agent: String,
    },
    /// Curate and summarize memory entries
    Curate {
        /// Synchronize curation with the index
        #[arg(long)]
        sync: bool,
        /// Summary text
        summary: String,
        /// Agent ID for Bulwark policy evaluation (default: "cli")
        #[arg(long, default_value = "cli")]
        agent: String,
    },
    /// Initialize a new Engram workspace
    Init {
        /// Path to the workspace root (default: current directory)
        #[arg(long)]
        workspace: Option<String>,
    },
    /// Query the memory index
    Query {
        /// The query string to search for
        query_string: Option<String>,
        /// Output format: text (default) or json
        #[arg(long, default_value = "text")]
        format: String,
        /// Agent ID for Bulwark policy evaluation (default: "cli")
        #[arg(long, default_value = "cli")]
        agent: String,
        /// Verify the audit log hash chain and exit
        #[arg(long)]
        verify_audit: bool,
        /// Path to the audit log (default: .brv/audit/engram.log)
        #[arg(long, default_value = ".brv/audit/engram.log")]
        log: String,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let root = std::env::current_dir()?;
    let brv_dir = root.join(".brv");
    let config = load_workspace_config(&brv_dir);

    // new_from_config handles missing bulwark.toml gracefully (allow-all fallback)
    let bulwark = BulwarkHandle::new_from_config(
        brv_dir.join("bulwark.toml"),
        Some(brv_dir.join("audit")),
        &config.audit,
    );

    bulwark.verify_siem_reachability()
        .unwrap_or_else(|e| {
            eprintln!("error: {}", e);
            std::process::exit(1);
        });

    let mut cache = ExactCache::new(config.exact_cache_ttl_secs);
    let mut fuzzy_cache = FuzzyCache::new(100);

    match cli.command {
        Commands::Compile { watch, classify, incremental, agent } => {
            std::env::set_var("ENGRAM_AGENT_ID", &agent);
            if watch {
                return engram_compiler::watcher::run_watch(&root, &config);
            }

            // CLI --classify flag overrides config; config is fallback
            let mut compile_config = config.compile.clone();
            if classify {
                compile_config.classify = true;
            }

            let result = if incremental {
                engram_compiler::compile_incremental(&root, &bulwark, &compile_config)
            } else {
                engram_compiler::compile_context_tree_with_config(
                    &root,
                    true,
                    &bulwark,
                    &compile_config,
                )
            };

            // Print parse summary
            println!(
                "Parsed {} files: {} succeeded, {} failed",
                result.parse_result.file_count,
                result.parse_result.records.len(),
                result.parse_result.error_count,
            );

            // Print parse warnings
            for warning in &result.parse_result.warnings {
                eprintln!("WARN: {}", warning.message);
            }

            // Print parse errors
            for error in &result.parse_result.errors {
                eprintln!("ERROR: {}", error);
            }

            // Print index stats or error
            if let Some(stats) = &result.index_stats {
                println!(
                    "Indexed {} documents in {}ms",
                    stats.documents_written, stats.elapsed_ms,
                );
            }
            if let Some(err) = &result.index_error {
                eprintln!("Index error: {}", err);
            }

            // Print manifest stats or error
            if let Some(stats) = &result.manifest_stats {
                println!(
                    "Manifest: {} entries, {} bytes",
                    stats.entries_written, stats.size_bytes,
                );
            }
            if let Some(err) = &result.manifest_error {
                eprintln!("Manifest error: {}", err);
            }

            // Print state
            if let Some(state) = &result.state {
                println!(
                    "Compiled {} files in {}ms (generation {})",
                    state.compiled_file_count,
                    state.last_compiled_duration_ms.unwrap_or(0),
                    state.generation,
                );
            }
            if let Some(err) = &result.state_error {
                eprintln!("State error: {}", err);
            }

            // Print causal graph summary
            if let Some(report) = &result.causal_report {
                if report.skipped_unchanged {
                    println!("Causal graph: skipped (unchanged)");
                } else {
                    println!(
                        "Causal graph: {} nodes, {} edges",
                        report.node_count, report.edge_count,
                    );
                }
            }

            // Print causal validation warnings
            for w in &result.causal_warnings {
                match w {
                    CausalValidationWarning::DanglingEdge { source_path, target_id, .. } => {
                        eprintln!(
                            "WARN [causal] {}: references unknown fact ID {:?} (no matching .md file found in context tree)",
                            source_path, target_id
                        );
                    }
                    CausalValidationWarning::SelfLoop { fact_id } => {
                        eprintln!("WARN [causal] self-loop: {}", fact_id);
                    }
                    CausalValidationWarning::CycleDetected { cycle_ids } => {
                        eprintln!("WARN [causal] cycle detected: {}", cycle_ids.join(" \u{2192} "));
                    }
                }
            }

            // Exit with error code if any hard failures occurred
            if result.index_error.is_some() {
                std::process::exit(1);
            }
        }
        Commands::Init { workspace } => {
            let ws_root = workspace
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| root.clone());
            let brv = ws_root.join(".brv");

            if brv.join("context-tree").exists() {
                println!("Engram workspace already initialized at .brv/");
                return Ok(());
            }

            // Create directory structure
            std::fs::create_dir_all(brv.join("context-tree"))?;
            std::fs::create_dir_all(brv.join("audit"))?;

            // Write default config if it doesn't exist
            let config_path = brv.join("engram.toml");
            if !config_path.exists() {
                std::fs::write(
                    &config_path,
                    "# Low threshold recommended for new workspaces with small corpora\n\
                     [query]\n\
                     score_threshold = 0.0\n\
                     \n\
                     [access_tracking]\n\
                     enabled = true\n\
                     importance_delta = 0.001\n",
                )?;
            }

            // Run initial compile to create an empty index
            let init_config = load_workspace_config(&brv);
            let init_bulwark = BulwarkHandle::new_from_config(
                brv.join("bulwark.toml"),
                Some(brv.join("audit")),
                &init_config.audit,
            );
            engram_compiler::compile_context_tree(&ws_root, true, &init_bulwark);

            println!("Engram workspace initialized at .brv/");
            println!(
                "Tip: add '.brv/index/' to your .gitignore (compiled index is a\n\
                 derived artifact). Keep '.brv/context-tree/' and '.brv/engram.toml'\n\
                 in version control."
            );
        }
        Commands::Curate { sync, summary, agent } => {
            std::env::set_var("ENGRAM_AGENT_ID", &agent);
            let options = engram_compiler::CurateOptions { summary, sync };

            match engram_compiler::curate(&root, options, &bulwark) {
                Ok(result) => {
                    println!("Curated: {}", result.written_path.display());
                    if let Some(compile) = &result.sync_compile_result {
                        if let Some(stats) = &compile.index_stats {
                            println!(
                                "Index updated: {} documents in {}ms",
                                stats.documents_written, stats.elapsed_ms,
                            );
                        }
                        if let Some(state) = &compile.state {
                            println!("Generation: {}", state.generation);
                        }
                    } else {
                        println!("Index update queued (async)");
                    }
                }
                Err(e) => {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
            }
        }
        Commands::Query { query_string, format, agent, verify_audit, log } => {
            if verify_audit {
                let log_path = std::path::Path::new(&log);
                match engram_bulwark::verify_audit_chain(log_path) {
                    Ok(n) => {
                        println!("Audit chain valid: {} entries", n);
                        return Ok(());
                    }
                    Err(e) => {
                        eprintln!("Audit chain verification failed: {:?}", e);
                        std::process::exit(1);
                    }
                }
            }

            let query_string = query_string.unwrap_or_else(|| {
                eprintln!("Error: query string required (or use --verify-audit)");
                std::process::exit(1);
            });

            let options = QueryOptions {
                agent_id: agent,
                ..QueryOptions::default()
            };

            let use_json = format == "json";

            match engram_query::query(&root, &query_string, options, &mut cache, &mut fuzzy_cache, &bulwark, &config) {
                Ok(result) => {
                    if result.meta.stale {
                        eprintln!(
                            "WARN: index is stale since {} — \
                            results may not reflect recent curations",
                            result
                                .meta
                                .dirty_since
                                .map(|t| t.to_rfc3339())
                                .unwrap_or_else(|| "unknown".to_string())
                        );
                    }
                    if use_json {
                        print_json_results(&root, &result);
                    } else if result.hits.is_empty() {
                        println!("No results found.");
                    } else {
                        println!(
                            "Found {} result(s) [tier {}, {}ms, gen {}]:",
                            result.hits.len(),
                            result.meta.cache_tier,
                            result.meta.query_ms,
                            result.meta.index_generation,
                        );
                        for (i, hit) in result.hits.iter().enumerate() {
                            println!(
                                "  {}. [score: {:.3}] {} ({})",
                                i + 1,
                                hit.score,
                                hit.title.as_deref().unwrap_or("<untitled>"),
                                hit.source_path,
                            );
                        }
                    }
                }
                Err(QueryError::IndexNotFound) => {
                    eprintln!(
                        "No compiled index found. \
                        Run 'engram compile' first."
                    );
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("Query error: {}", e);
                    std::process::exit(1);
                }
            }
        }
    }

    Ok(())
}

/// Read body text from a fact's source `.md` file.
///
/// This is intentional display-layer file I/O: the query library does not
/// store body text in the Tantivy index (NRD-4), so the CLI reads it from
/// the source file at output time for `--format json`.
fn read_body(root: &std::path::Path, source_path: &str) -> String {
    let path = root.join(source_path);
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return String::new(),
    };

    // Strip YAML frontmatter (lines between first and second `---`)
    let mut lines = content.lines();
    let mut body_lines: Vec<&str> = Vec::new();
    let mut in_frontmatter = false;
    let mut frontmatter_ended = false;

    for line in &mut lines {
        if !frontmatter_ended {
            if line.trim() == "---" {
                if !in_frontmatter {
                    in_frontmatter = true;
                    continue;
                } else {
                    frontmatter_ended = true;
                    continue;
                }
            }
            if in_frontmatter {
                continue;
            }
        }
        body_lines.push(line);
    }

    let body = body_lines.join("\n");

    // Strip leading blank lines
    let body = body.trim_start_matches('\n');

    // Strip `## Raw Concept` header if it's the first non-blank line
    let body = if body.starts_with("## Raw Concept") {
        body.strip_prefix("## Raw Concept")
            .unwrap_or(body)
            .trim_start_matches('\n')
    } else {
        body
    };

    let body = body.trim();

    // Truncate to 500 chars at last sentence boundary
    if body.len() <= 500 {
        return body.to_string();
    }

    let truncated = &body[..500];
    // Find last sentence-ending punctuation
    if let Some(pos) = truncated.rfind(|c| c == '.' || c == '!' || c == '?') {
        format!("{}...", &truncated[..=pos])
    } else {
        // No sentence boundary found — truncate at last space
        if let Some(pos) = truncated.rfind(' ') {
            format!("{}...", &truncated[..pos])
        } else {
            format!("{}...", truncated)
        }
    }
}

/// Extract fact_id from source_path (filename stem).
fn fact_id_from_path(source_path: &str) -> String {
    std::path::Path::new(source_path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Print query results as NDJSON (one JSON object per line).
fn print_json_results(root: &std::path::Path, result: &engram_query::QueryResult) {
    for (i, hit) in result.hits.iter().enumerate() {
        let body = read_body(root, &hit.source_path);
        let fact_id = fact_id_from_path(&hit.source_path);

        let obj = serde_json::json!({
            "rank": i + 1,
            "score": hit.score,
            "title": hit.title.as_deref().unwrap_or(""),
            "fact_id": fact_id,
            "source_path": hit.source_path,
            "fact_type": hit.fact_type,
            "tags": hit.tags,
            "body": body,
            "cache_tier": result.meta.cache_tier,
            "answer": hit.answer,
        });

        println!("{}", obj);
    }
}
