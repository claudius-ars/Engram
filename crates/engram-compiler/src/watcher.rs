use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use engram_bulwark::BulwarkHandle;
use engram_core::WorkspaceConfig;

use crate::fingerprint::{
    compute_changes, detect_rename, make_fingerprint,
    save_fingerprints, FingerprintEnvelope,
};
use crate::indexer::{incremental_update, open_index};
use crate::parser::parse_file;
use crate::walker::walk_context_tree;

/// Debounce window for collecting filesystem events (milliseconds).
pub const WATCH_DEBOUNCE_MS: u64 = 50;

/// Run the watch loop. Blocks until Ctrl-C or SIGTERM.
/// Performs an initial full compile, then watches for changes.
pub fn run_watch(
    workspace_root: &Path,
    _config: &WorkspaceConfig,
) -> anyhow::Result<()> {
    let bulwark = BulwarkHandle::new_stub();

    // Step 1: Initial compile (full rebuild)
    eprintln!("watch: initial compile...");
    let result = crate::compile_context_tree(workspace_root, true, &bulwark);

    if let Some(err) = &result.index_error {
        anyhow::bail!("initial compile failed: {}", err);
    }

    let index_dir = workspace_root.join(".brv").join("index");

    // Build initial fingerprints from the compile result
    let generation = result.state.as_ref().map(|s| s.generation).unwrap_or(1);
    let mut fingerprints = FingerprintEnvelope::new();
    let paths = walk_context_tree(workspace_root).unwrap_or_default();
    for path in &paths {
        if let Ok(fp) = make_fingerprint(path, workspace_root, 1, generation) {
            fingerprints.entries.insert(fp.source_path.clone(), fp);
        }
    }
    save_fingerprints(&index_dir, &fingerprints);

    eprintln!(
        "watch: initial compile done ({} files, generation {})",
        fingerprints.entries.len(),
        generation,
    );

    // Step 2: Open the index for incremental updates (keep writer alive)
    let (index, schema) = open_index(workspace_root)?;
    let mut index_writer = index.writer(50_000_000)?;

    // Step 3: Set up signal handler
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();
    ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
    })?;

    // Step 4: Start notify watcher on context-tree
    let context_tree = workspace_root.join(".brv").join("context-tree");
    if !context_tree.exists() {
        anyhow::bail!("context-tree not found: {}", context_tree.display());
    }

    let (tx, rx) = mpsc::channel::<Event>();
    let mut watcher: RecommendedWatcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
        if let Ok(event) = res {
            let _ = tx.send(event);
        }
    })?;

    watcher.watch(&context_tree, RecursiveMode::Recursive)?;
    eprintln!("watch: watching {} for changes...", context_tree.display());

    let mut current_generation = generation;

    // Step 5: Main event loop
    while running.load(Ordering::SeqCst) {
        // Collect events for the debounce window
        let mut events = Vec::new();

        // Wait for first event (blocking, with timeout so we can check `running`)
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(event) => events.push(event),
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        if !running.load(Ordering::SeqCst) {
            break;
        }

        // Collect remaining events within debounce window
        let debounce_deadline =
            std::time::Instant::now() + Duration::from_millis(WATCH_DEBOUNCE_MS);
        loop {
            let remaining = debounce_deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match rx.recv_timeout(remaining) {
                Ok(event) => events.push(event),
                Err(_) => break,
            }
        }

        // Classify events into affected paths
        let mut affected_paths = std::collections::HashSet::new();
        for event in &events {
            match event.kind {
                EventKind::Create(_)
                | EventKind::Modify(_)
                | EventKind::Remove(_) => {
                    for path in &event.paths {
                        if path.extension().map(|e| e == "md").unwrap_or(false) {
                            affected_paths.insert(path.clone());
                        }
                    }
                }
                _ => {}
            }
        }

        if affected_paths.is_empty() {
            continue;
        }

        // Reload current file list
        let current_files = match walk_context_tree(workspace_root) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("watch: failed to walk context tree: {}", e);
                continue;
            }
        };

        // Compute changes
        let (changes, mtime_updates) =
            match compute_changes(&fingerprints, &current_files, workspace_root) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("watch: failed to compute changes: {}", e);
                    continue;
                }
            };

        // Apply mtime-only updates
        for update in &mtime_updates {
            if let Some(fp) = fingerprints.entries.get_mut(&update.rel_path) {
                fp.mtime_secs = update.new_mtime_secs;
                fp.mtime_nanos = update.new_mtime_nanos;
            }
        }

        if changes.is_empty() {
            if !mtime_updates.is_empty() {
                save_fingerprints(&index_dir, &fingerprints);
            }
            continue;
        }

        let start = std::time::Instant::now();

        // Handle renames: for each added file, check if it's a rename of a deleted file
        let mut rename_map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        for added_path in &changes.added {
            let rel = added_path
                .strip_prefix(workspace_root)
                .unwrap_or(added_path)
                .to_string_lossy()
                .to_string();
            if let Ok(Some(old_rel)) =
                detect_rename(&fingerprints, Path::new(&rel), &changes.deleted, workspace_root)
            {
                rename_map.insert(rel, old_rel);
            }
        }

        // Build deletions list: deleted files + modified files (old versions)
        // Build deletions with index_source_path (matching Tantivy storage)
        let mut deletions: Vec<String> = Vec::new();
        for del in &changes.deleted {
            if let Some(fp) = fingerprints.entries.get(del) {
                deletions.push(fp.index_source_path.clone());
            }
        }
        for mod_path in &changes.modified {
            let rel = mod_path
                .strip_prefix(workspace_root)
                .unwrap_or(mod_path)
                .to_string_lossy()
                .to_string();
            if let Some(fp) = fingerprints.entries.get(&rel) {
                deletions.push(fp.index_source_path.clone());
            } else {
                deletions.push(mod_path.to_string_lossy().to_string());
            }
        }
        // Remove renamed files from deletions (the rename target handles the add)
        for old_rel in rename_map.values() {
            if let Some(fp) = fingerprints.entries.get(old_rel) {
                let abs_old = fp.index_source_path.clone();
                deletions.retain(|d| d != &abs_old);
            }
        }

        // Parse added + modified files
        let mut additions = Vec::new();
        let files_to_parse: Vec<&PathBuf> = changes
            .added
            .iter()
            .chain(changes.modified.iter())
            .collect();

        for path in &files_to_parse {
            match parse_file(path) {
                Ok(record) => additions.push(record),
                Err(e) => {
                    eprintln!("watch: parse error: {}", e);
                }
            }
        }

        // Perform incremental update
        match incremental_update(&schema, &mut index_writer, &deletions, &additions) {
            Ok(stats) => {
                current_generation += 1;

                // Update fingerprints: remove deleted, add/update changed
                for del in &changes.deleted {
                    fingerprints.entries.remove(del);
                }
                for path in files_to_parse {
                    if let Ok(fp) =
                        make_fingerprint(path, workspace_root, 1, current_generation)
                    {
                        fingerprints.entries.insert(fp.source_path.clone(), fp);
                    }
                }
                save_fingerprints(&index_dir, &fingerprints);

                // Update state
                let duration_ms = start.elapsed().as_millis() as u64;
                let mut new_state = crate::fresh_state(
                    fingerprints.entries.len() as u64,
                    duration_ms,
                );
                new_state.generation = current_generation;
                let _ = crate::write_state(&index_dir, &new_state);

                eprintln!(
                    "watch: reindexed {} file(s) in {}ms (generation {})",
                    stats.documents_written, stats.elapsed_ms, current_generation,
                );
            }
            Err(e) => {
                eprintln!("watch: incremental update failed: {}", e);
            }
        }
    }

    // Shutdown: save fingerprints
    save_fingerprints(&index_dir, &fingerprints);
    eprintln!("watch: shutting down");

    Ok(())
}
