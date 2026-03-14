use std::path::{Path, PathBuf};

use chrono::Utc;
use engram_bulwark::{
    AccessType, BulwarkHandle, PolicyDecision, PolicyRequest,
};

use crate::state::{read_state, write_state, IndexState, StateError, COMPILER_VERSION, STATE_VERSION};
use crate::{compile_context_tree, CompileResult};

pub struct CurateOptions {
    pub summary: String,
    pub sync: bool,
}

pub struct CurateResult {
    pub written_path: PathBuf,
    pub slug: String,
    pub sync_compile_result: Option<CompileResult>,
}

#[derive(Debug, thiserror::Error)]
pub enum CurateError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("state error: {0}")]
    State(#[from] StateError),

    #[error("lock error: {0}")]
    Lock(String),

    #[error("summary is empty")]
    EmptySummary,

    #[error("policy denied: {0}")]
    PolicyDenied(String),
}

/// Derives a URL-safe slug from the first 6 words of a summary.
pub fn make_slug(summary: &str) -> Result<String, CurateError> {
    let trimmed = summary.trim();
    if trimmed.is_empty() {
        return Err(CurateError::EmptySummary);
    }

    let words: Vec<&str> = trimmed.split_whitespace().take(6).collect();
    let joined = words.join("-").to_lowercase();

    let slug: String = joined
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
        .collect();

    // Collapse consecutive hyphens
    let mut result = String::with_capacity(slug.len());
    let mut prev_hyphen = false;
    for c in slug.chars() {
        if c == '-' {
            if !prev_hyphen {
                result.push(c);
            }
            prev_hyphen = true;
        } else {
            result.push(c);
            prev_hyphen = false;
        }
    }

    // Trim trailing hyphens and truncate to 60 chars
    let result = result.trim_end_matches('-').to_string();
    let result = if result.len() > 60 {
        result[..60].trim_end_matches('-').to_string()
    } else {
        result
    };

    Ok(result)
}

/// Determines a unique output path for the curated .md file.
fn determine_output_path(root: &Path, slug: &str) -> PathBuf {
    let curated_dir = root.join(".brv").join("context-tree").join("curated");
    let date = Utc::now().format("%Y-%m-%d").to_string();

    let base = curated_dir.join(format!("{}-{}.md", date, slug));
    if !base.exists() {
        return base;
    }

    let mut counter = 2u32;
    loop {
        let candidate = curated_dir.join(format!("{}-{}-{}.md", date, slug, counter));
        if !candidate.exists() {
            return candidate;
        }
        counter += 1;
    }
}

/// Generates the .md file content for a curated fact.
fn generate_md_content(summary: &str) -> String {
    let now = Utc::now().to_rfc3339();
    let title: String = summary.chars().take(80).collect();

    format!(
        "---\ntitle: \"{}\"\nfactType: durable\nconfidence: 1.0\ncreatedAt: \"{}\"\nupdatedAt: \"{}\"\n---\n\n## Raw Concept\n\n{}\n\n## Metadata\n\n_Curated by engram at {}_\n",
        title, now, now, summary, now
    )
}

/// Sets the dirty flag in the state file.
fn set_dirty(index_dir: &Path) -> Result<(), StateError> {
    std::fs::create_dir_all(index_dir).map_err(StateError::Io)?;

    let mut state = match read_state(index_dir) {
        Ok(s) => s,
        Err(_) => {
            // No state file yet — create a minimal dirty state
            IndexState {
                version: STATE_VERSION,
                dirty: true,
                generation: 0,
                dirty_since: Some(Utc::now()),
                last_compiled_at: None,
                last_compiled_duration_ms: None,
                compiled_file_count: 0,
                compiler_version: COMPILER_VERSION.to_string(),
            }
        }
    };

    state.dirty = true;
    state.dirty_since = Some(Utc::now());
    write_state(index_dir, &state)
}

// --- Lock file protocol ---

fn lock_path(index_dir: &Path) -> PathBuf {
    index_dir.join("compile.lock")
}

fn try_acquire_lock(index_dir: &Path) -> Result<bool, CurateError> {
    let lock = lock_path(index_dir);
    std::fs::create_dir_all(index_dir).map_err(CurateError::Io)?;

    if !lock.exists() {
        std::fs::write(&lock, std::process::id().to_string())?;
        return Ok(true);
    }

    let content = std::fs::read_to_string(&lock)?;
    let pid: i32 = match content.trim().parse() {
        Ok(p) => p,
        Err(_) => {
            // Unparseable PID — treat as stale
            std::fs::write(&lock, std::process::id().to_string())?;
            return Ok(true);
        }
    };

    // Check if process is alive via kill(pid, 0)
    let alive = unsafe { libc::kill(pid, 0) } == 0;
    if alive {
        Ok(false)
    } else {
        // Stale lock — overwrite
        std::fs::write(&lock, std::process::id().to_string())?;
        Ok(true)
    }
}

fn release_lock(index_dir: &Path) {
    let _ = std::fs::remove_file(lock_path(index_dir));
}

struct LockGuard {
    index_dir: PathBuf,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        release_lock(&self.index_dir);
    }
}

/// Spawns a background compile process.
fn spawn_background_compile(root: &Path, index_dir: &Path) -> Result<(), CurateError> {
    let acquired = try_acquire_lock(index_dir)?;
    if !acquired {
        eprintln!("engram: compile already in progress, skipping");
        return Ok(());
    }

    let exe = std::env::current_exe()?;
    let child = std::process::Command::new(exe)
        .arg("compile")
        .current_dir(root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;

    // Write child PID to lock file so other curate calls see it
    std::fs::write(lock_path(index_dir), child.id().to_string())?;

    Ok(())
}

/// Top-level curate function called by the CLI.
pub fn curate(root: &Path, options: CurateOptions, bulwark: &BulwarkHandle) -> Result<CurateResult, CurateError> {
    // Policy check
    let request = PolicyRequest {
        access_type: AccessType::Write,
        fact_id: None,
        agent_id: None,
        operation: "curate".to_string(),
        domain_tags: vec![],
        fact_types: vec!["durable".to_string()],
    };

    let t0 = std::time::Instant::now();
    let decision = bulwark.check(&request);
    let duration_ms = t0.elapsed().as_millis() as u64;
    bulwark.audit(&request, &decision, duration_ms);
    if let PolicyDecision::Deny { reason, .. } = decision {
        return Err(CurateError::PolicyDenied(reason));
    }

    let summary = options.summary.trim().to_string();
    if summary.is_empty() {
        return Err(CurateError::EmptySummary);
    }

    let slug = make_slug(&summary)?;

    // Phase A: Write .md file
    let output_path = determine_output_path(root, &slug);
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = generate_md_content(&summary);
    std::fs::write(&output_path, &content)?;

    // Set dirty flag
    let index_dir = root.join(".brv").join("index");
    set_dirty(&index_dir)?;

    // Phase B: Compile
    if options.sync {
        // --sync path: blocking in-process compile
        let acquired = try_acquire_lock(&index_dir)?;
        if !acquired {
            return Err(CurateError::Lock(
                "compile already in progress — retry after current compile completes".to_string(),
            ));
        }
        let _guard = LockGuard {
            index_dir: index_dir.clone(),
        };

        let compile_result = compile_context_tree(root, true, bulwark);

        // Verify consistency
        if let Ok(state) = read_state(&index_dir) {
            if state.dirty || state.generation == 0 {
                return Err(CurateError::Lock(
                    "compile completed but index state is inconsistent".to_string(),
                ));
            }
        }

        Ok(CurateResult {
            written_path: output_path,
            slug,
            sync_compile_result: Some(compile_result),
        })
    } else {
        // Async path: spawn background compile
        spawn_background_compile(root, &index_dir)?;

        Ok(CurateResult {
            written_path: output_path,
            slug,
            sync_compile_result: None,
        })
    }
}
