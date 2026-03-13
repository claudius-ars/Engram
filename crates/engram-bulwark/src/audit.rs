use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::policy::{AccessType, PolicyDecision, PolicyRequest};

/// Internal audit event serialized as NDJSON to the audit log.
#[derive(Debug, Serialize)]
pub struct AuditEvent {
    pub ts_ms: i64,
    pub agent_id: String,
    pub operation: String,
    pub access_type: String,
    pub fact_ids: Vec<String>,
    pub domain_tags: Vec<String>,
    pub decision: String,
    pub reason: Option<String>,
    pub rule_name: Option<String>,
    pub duration_ms: u64,
    pub prev_hash: String,
}

/// Append-only audit log writer with SHA-256 hash chain.
#[derive(Debug)]
pub struct AuditWriter {
    log_path: PathBuf,
}

impl AuditWriter {
    pub fn new(log_path: PathBuf) -> Self {
        AuditWriter { log_path }
    }

    /// Append an audit entry to the log.
    ///
    /// Opens the file with create + read + append, acquires an exclusive lock,
    /// computes the hash chain, writes the NDJSON line, and releases the lock.
    pub fn append(
        &mut self,
        request: &PolicyRequest,
        decision: &PolicyDecision,
        duration_ms: u64,
    ) -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&self.log_path)?;

        // Exclusive lock required — NFS does not guarantee write atomicity.
        file.lock_exclusive()?;

        let prev_hash = compute_prev_hash_from_file(&mut file)?;

        let (decision_str, reason, rule_name) = match decision {
            PolicyDecision::Allow => ("allow".to_string(), None, None),
            PolicyDecision::Deny { reason, rule_name } => {
                ("deny".to_string(), Some(reason.clone()), rule_name.clone())
            }
        };

        let access_type_str = match request.access_type {
            AccessType::Read => "Read",
            AccessType::Write => "Write",
            AccessType::LlmCall => "LlmCall",
        };

        let event = AuditEvent {
            ts_ms: chrono::Utc::now().timestamp_millis(),
            agent_id: request.agent_id.clone().unwrap_or_default(),
            operation: request.operation.clone(),
            access_type: access_type_str.to_string(),
            fact_ids: request.fact_id.iter().cloned().collect(),
            domain_tags: vec![],
            decision: decision_str,
            reason,
            rule_name,
            duration_ms,
            prev_hash,
        };

        let json = serde_json::to_string(&event).map_err(std::io::Error::other)?;

        // Seek to end (append mode does this, but be explicit after read)
        file.seek(SeekFrom::End(0))?;
        writeln!(file, "{}", json)?;
        file.flush()?;

        file.unlock()?;
        Ok(())
    }
}

/// Compute the SHA-256 hex hash of the last complete line in the file.
/// Returns 64 hex zeros if the file is empty or has no complete lines.
fn compute_prev_hash_from_file(file: &mut File) -> std::io::Result<String> {
    let mut content = String::new();
    file.seek(SeekFrom::Start(0))?;
    file.read_to_string(&mut content)?;

    if content.is_empty() {
        return Ok("0".repeat(64));
    }

    // Handle partial last line (crash mid-write)
    if !content.ends_with('\n') {
        eprintln!("WARN [bulwark] audit log has partial last line — using last complete line for hash chain");
    }

    let last_complete_line = content
        .lines()
        .rev()
        .find(|line| !line.is_empty());

    match last_complete_line {
        Some(line) => {
            let line_with_newline = format!("{}\n", line);
            let hash = Sha256::digest(line_with_newline.as_bytes());
            Ok(hex::encode(hash))
        }
        None => Ok("0".repeat(64)),
    }
}

/// Error type for audit chain verification.
#[derive(Debug)]
pub enum ChainError {
    Io(std::io::Error),
    HashMismatch {
        line_number: usize,
        expected_hash: String,
        stored_hash: String,
    },
    InvalidJson {
        line_number: usize,
        error: String,
    },
}

impl From<std::io::Error> for ChainError {
    fn from(e: std::io::Error) -> Self {
        ChainError::Io(e)
    }
}

/// Verify the SHA-256 hash chain of an audit log.
///
/// Returns `Ok(entry_count)` if all entries are valid and the chain is intact.
/// Returns `Err(ChainError)` on the first broken link.
///
/// Partial last lines (no trailing newline) are excluded from verification
/// and do not cause an error.
pub fn verify_audit_chain(log_path: &Path) -> Result<usize, ChainError> {
    let content = std::fs::read_to_string(log_path)?;

    if content.is_empty() {
        return Ok(0);
    }

    // Detect partial last line
    let has_partial = !content.ends_with('\n');
    if has_partial {
        eprintln!("WARN [bulwark] audit log has partial last line — verifying only complete entries");
    }

    let lines: Vec<&str> = content.lines().filter(|l| !l.is_empty()).collect();

    // If there's a partial last line, exclude it
    let verify_count = if has_partial && !lines.is_empty() {
        lines.len() - 1
    } else {
        lines.len()
    };

    let mut expected_hash = "0".repeat(64);
    let mut count = 0;

    for (i, &line) in lines.iter().take(verify_count).enumerate() {
        let line_number = i + 1;

        let value: serde_json::Value = serde_json::from_str(line).map_err(|e| {
            ChainError::InvalidJson {
                line_number,
                error: e.to_string(),
            }
        })?;

        let stored_hash = value
            .get("prev_hash")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ChainError::InvalidJson {
                line_number,
                error: "missing prev_hash field".to_string(),
            })?;

        if stored_hash != expected_hash {
            return Err(ChainError::HashMismatch {
                line_number,
                expected_hash,
                stored_hash: stored_hash.to_string(),
            });
        }

        // Compute hash of this line (including trailing newline) for next iteration
        let line_with_newline = format!("{}\n", line);
        let hash = Sha256::digest(line_with_newline.as_bytes());
        expected_hash = hex::encode(hash);
        count += 1;
    }

    Ok(count)
}

// Re-export the old types that downstream crates expect.
// AuditOutcome is still used in the public API.
#[derive(Debug, Clone)]
pub enum AuditOutcome {
    Success,
    Failure { reason: String },
}
