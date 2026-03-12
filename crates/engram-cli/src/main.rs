use clap::{Parser, Subcommand};
use engram_bulwark::BulwarkHandle;
use engram_core::load_workspace_config;
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
    },
    /// Curate and summarize memory entries
    Curate {
        /// Synchronize curation with the index
        #[arg(long)]
        sync: bool,
        /// Summary text
        summary: String,
    },
    /// Query the memory index
    Query {
        /// The query string to search for
        query_string: String,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let bulwark = BulwarkHandle::new_stub();
    let root = std::env::current_dir()?;
    let config = load_workspace_config(&root.join(".brv"));
    let mut cache = ExactCache::new(config.exact_cache_ttl_secs);
    let mut fuzzy_cache = FuzzyCache::new(100);

    match cli.command {
        Commands::Compile { watch, classify, incremental } => {
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

            // Exit with error code if any hard failures occurred
            if result.index_error.is_some() {
                std::process::exit(1);
            }
        }
        Commands::Curate { sync, summary } => {
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
        Commands::Query { query_string } => {
            let options = QueryOptions::default();

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
                    if result.hits.is_empty() {
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
