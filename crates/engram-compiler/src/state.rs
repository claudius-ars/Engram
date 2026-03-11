use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const STATE_VERSION: u32 = 1;
pub const COMPILER_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexState {
    pub version: u32,
    pub dirty: bool,
    pub generation: u64,
    pub dirty_since: Option<DateTime<Utc>>,
    pub last_compiled_at: Option<DateTime<Utc>>,
    pub last_compiled_duration_ms: Option<u64>,
    pub compiled_file_count: u64,
    pub compiler_version: String,
}

#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

fn state_path(index_dir: &Path) -> PathBuf {
    index_dir.join("state")
}

fn state_tmp_path(index_dir: &Path) -> PathBuf {
    index_dir.join("state.tmp")
}

pub fn read_state(index_dir: &Path) -> Result<IndexState, StateError> {
    let content = std::fs::read_to_string(state_path(index_dir))?;
    let state: IndexState = serde_json::from_str(&content)?;
    Ok(state)
}

pub fn write_state(index_dir: &Path, state: &IndexState) -> Result<(), StateError> {
    let json = serde_json::to_string_pretty(state)?;
    let tmp = state_tmp_path(index_dir);
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, state_path(index_dir))?;
    Ok(())
}

pub fn fresh_state(compiled_file_count: u64, duration_ms: u64) -> IndexState {
    IndexState {
        version: STATE_VERSION,
        dirty: false,
        generation: 1,
        dirty_since: None,
        last_compiled_at: Some(Utc::now()),
        last_compiled_duration_ms: Some(duration_ms),
        compiled_file_count,
        compiler_version: COMPILER_VERSION.to_string(),
    }
}
