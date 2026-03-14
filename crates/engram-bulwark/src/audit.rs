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

/// Append-only audit log writer with SHA-256 hash chain, size-based rotation,
/// and optional SIEM emission.
#[derive(Debug)]
pub struct AuditWriter {
    log_path: PathBuf,
    /// Maximum log size in bytes before rotation. 0 = no rotation.
    pub max_log_bytes: u64,
    /// SIEM endpoint URL. None = SIEM disabled.
    siem_endpoint: Option<String>,
    /// Resolved bearer token value. Never logged.
    siem_token: Option<String>,
    /// If true, startup fails when SIEM endpoint is unreachable.
    siem_required: bool,
}

impl AuditWriter {
    /// Create a new audit writer.
    ///
    /// `siem_token_env`: if Some, the named env var is read at construction time
    /// to resolve the bearer token. If the env var is not set, SIEM emission is
    /// silently disabled for this writer instance.
    pub fn new(
        log_path: PathBuf,
        max_log_bytes: u64,
        siem_endpoint: Option<String>,
        siem_token_env: Option<&str>,
        siem_required: bool,
    ) -> Self {
        let siem_token = siem_token_env.and_then(|env_var| {
            match std::env::var(env_var) {
                Ok(val) => Some(val),
                Err(_) => {
                    eprintln!(
                        "WARN [bulwark] SIEM token env var '{}' not set — SIEM emission disabled",
                        env_var
                    );
                    None
                }
            }
        });

        AuditWriter {
            log_path,
            max_log_bytes,
            siem_endpoint,
            siem_token,
            siem_required,
        }
    }

    /// Verifies SIEM endpoint reachability with a HEAD request.
    /// Returns Ok(()) if SIEM is not configured or endpoint is reachable.
    /// Returns Err(String) if siem_required = true and endpoint is unreachable.
    pub fn verify_siem_reachability(&self) -> Result<(), String> {
        let url = match &self.siem_endpoint {
            None => return Ok(()),
            Some(u) => u.clone(),
        };

        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .map_err(|e| format!("SIEM client error: {}", e))?;

        let mut req = client.head(&url);
        if let Some(token) = &self.siem_token {
            req = req.bearer_auth(token);
        }

        match req.send() {
            Ok(resp) if resp.status().is_success() || resp.status().is_redirection() => Ok(()),
            Ok(resp) => {
                let reason = format!("HTTP {}", resp.status());
                self.handle_unreachable(&url, &reason)
            }
            Err(e) => self.handle_unreachable(&url, &e.to_string()),
        }
    }

    fn handle_unreachable(&self, url: &str, reason: &str) -> Result<(), String> {
        if self.siem_required {
            Err(format!("SIEM endpoint unreachable: {}: {}", url, reason))
        } else {
            eprintln!("WARN [bulwark] SIEM endpoint unreachable: {}: {}", url, reason);
            Ok(())
        }
    }

    /// Append an audit entry to the log.
    ///
    /// If `max_log_bytes > 0` and the current log exceeds the threshold,
    /// the log is rotated (renamed to an archive) before writing. Each
    /// rotated archive is an independent sealed chain (NRD-27).
    pub fn append(
        &mut self,
        request: &PolicyRequest,
        decision: &PolicyDecision,
        duration_ms: u64,
    ) -> std::io::Result<()> {
        // Check rotation before opening for append
        self.maybe_rotate()?;

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
            domain_tags: request.domain_tags.clone(),
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

        // SIEM emission is best-effort — never blocks or fails the audit append.
        self.emit_to_siem(&json);

        Ok(())
    }

    /// Emit an audit event to the configured SIEM endpoint.
    ///
    /// Best-effort: one retry on 5xx, no retry on 4xx or network error.
    /// Failures are logged to stderr but never propagated.
    fn emit_to_siem(&self, event_json: &str) {
        let (endpoint, token) = match (&self.siem_endpoint, &self.siem_token) {
            (Some(e), Some(t)) => (e, t),
            _ => return,
        };

        let client = reqwest::blocking::Client::new();
        let result = client
            .post(endpoint)
            .bearer_auth(token)
            .header("Content-Type", "application/json")
            .body(event_json.to_string())
            .send();

        match result {
            Ok(resp) if resp.status().is_server_error() => {
                eprintln!("WARN [bulwark] SIEM emit returned {}, retrying once", resp.status());
                let _ = client
                    .post(endpoint)
                    .bearer_auth(token)
                    .header("Content-Type", "application/json")
                    .body(event_json.to_string())
                    .send()
                    .map_err(|e| eprintln!("WARN [bulwark] SIEM retry failed: {}", e));
            }
            Ok(_) => {} // 2xx or 4xx — no retry on 4xx
            Err(e) => {
                eprintln!("WARN [bulwark] SIEM emit failed: {}", e);
            }
        }
    }

    /// Rotate the log file if it exceeds `max_log_bytes`.
    ///
    /// Archive name: `engram.log.YYYYMMDD-HHMMSS` (UTC).
    /// Collision avoidance: appends `.1`, `.2`, etc. if the target exists.
    fn maybe_rotate(&self) -> std::io::Result<()> {
        if self.max_log_bytes == 0 {
            return Ok(());
        }

        let meta = match std::fs::metadata(&self.log_path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };

        if meta.len() < self.max_log_bytes {
            return Ok(());
        }

        let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S");
        let base_archive = self.log_path.with_file_name(format!("engram.log.{}", ts));

        let archive_path = if !base_archive.exists() {
            base_archive
        } else {
            let mut counter = 1u32;
            loop {
                let candidate = self.log_path.with_file_name(
                    format!("engram.log.{}.{}", ts, counter),
                );
                if !candidate.exists() {
                    break candidate;
                }
                counter += 1;
            }
        };

        std::fs::rename(&self.log_path, &archive_path)?;
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

