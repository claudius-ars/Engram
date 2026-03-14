pub mod audit;
pub mod policy;
pub mod rules;

pub use audit::{AuditEvent, AuditWriter, ChainError, verify_audit_chain};
pub use policy::{AccessType, PolicyDecision, PolicyRequest};
pub use rules::{evaluate_policy, load_policy_file, PolicyFile, PolicyRule, PolicyState};

use engram_core::AuditConfig;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::Duration;

/// Handle to the Bulwark policy engine.
///
/// Three construction modes:
/// - `new_stub()` — allow-all (no file, tests & default)
/// - `new_denying()` — deny-all (tests only)
/// - `new_from_config(policy_path, audit_dir)` — real file-backed policy with hot-reload
#[derive(Debug, Clone)]
pub struct BulwarkHandle {
    state: Arc<RwLock<PolicyState>>,
    /// None for stub/denying; Some for file-backed configs.
    policy_path: Option<PathBuf>,
    /// None for stub/denying; Some when audit_dir is provided.
    audit_writer: Option<Arc<Mutex<AuditWriter>>>,
}

impl BulwarkHandle {
    /// Create a no-op stub handle. All requests are allowed.
    pub fn new_stub() -> Self {
        BulwarkHandle {
            state: Arc::new(RwLock::new(PolicyState::allow_all())),
            policy_path: None,
            audit_writer: None,
        }
    }

    /// Create a deny-all handle for testing policy denial paths.
    pub fn new_denying() -> Self {
        BulwarkHandle {
            state: Arc::new(RwLock::new(PolicyState::deny_all())),
            policy_path: None,
            audit_writer: None,
        }
    }

    /// Create a handle backed by a TOML policy file.
    ///
    /// - If the file does not exist: allow-all.
    /// - If the file is invalid TOML: deny-all failsafe.
    /// - Spawns a background thread that polls the file every 30s for changes.
    /// - If `audit_dir` is Some, creates the directory and initializes an AuditWriter.
    pub fn new_from_config(policy_path: PathBuf, audit_dir: Option<PathBuf>, audit_config: &AuditConfig) -> Self {
        let initial_state = load_policy_file(&policy_path);
        let state = Arc::new(RwLock::new(initial_state));

        // Shared flag: set to true when an immediate reload is requested.
        // On Unix, SIGHUP sets this flag via signal-hook.
        let reload_flag = Arc::new(AtomicBool::new(false));

        // Register SIGHUP handler (Unix only)
        #[cfg(unix)]
        {
            let flag = Arc::clone(&reload_flag);
            signal_hook::flag::register(signal_hook::consts::SIGHUP, flag)
                .expect("failed to register SIGHUP handler");
        }

        // Spawn hot-reload thread.
        // Sleeps 1s per tick; checks the AtomicBool flag every tick for
        // signal-triggered reloads, and checks file mtime every 30 ticks
        // for poll-based reloads (preserving Windows behavior).
        let reload_state = Arc::clone(&state);
        let reload_path = policy_path.clone();
        let flag_for_thread = Arc::clone(&reload_flag);
        thread::spawn(move || {
            let mut last_content = std::fs::read_to_string(&reload_path).ok();
            let mut tick: u32 = 0;
            loop {
                thread::sleep(Duration::from_secs(1));
                tick = tick.wrapping_add(1);

                let signal_fired = flag_for_thread.swap(false, Ordering::SeqCst);
                let poll_due = tick.is_multiple_of(30);

                if signal_fired || poll_due {
                    let current_content = std::fs::read_to_string(&reload_path).ok();
                    if current_content != last_content {
                        let new_state = load_policy_file(&reload_path);
                        if let Ok(mut guard) = reload_state.write() {
                            *guard = new_state;
                        }
                        last_content = current_content;
                    }
                }
            }
        });

        // Initialize audit writer if audit_dir is provided
        let audit_writer = audit_dir.and_then(|dir| {
            if let Err(e) = std::fs::create_dir_all(&dir) {
                eprintln!("WARN [bulwark] cannot create audit dir {}: {} — audit disabled", dir.display(), e);
                return None;
            }
            let log_path = dir.join("engram.log");
            Some(Arc::new(Mutex::new(AuditWriter::new(
                log_path,
                audit_config.max_log_bytes,
                audit_config.siem_endpoint.clone(),
                audit_config.siem_token_env.as_deref(),
                audit_config.siem_required,
            ))))
        });

        BulwarkHandle {
            state,
            policy_path: Some(policy_path),
            audit_writer,
        }
    }

    /// Evaluate a policy request against the current rules.
    pub fn check(&self, request: &PolicyRequest) -> PolicyDecision {
        let state = self.state.read().expect("policy state lock poisoned");
        evaluate_policy(&state, request)
    }

    /// Record an audit event to the append-only audit log.
    ///
    /// Non-fatal: write failures are logged to stderr but never propagated.
    /// Stub and denying handles silently skip (no audit_writer).
    pub fn audit(&self, request: &PolicyRequest, decision: &PolicyDecision, duration_ms: u64) {
        if let Some(ref writer) = self.audit_writer {
            if let Err(e) = writer.lock().expect("audit writer lock poisoned").append(request, decision, duration_ms) {
                eprintln!("ERROR [bulwark] audit write failed: {}", e);
            }
        }
    }

    /// Returns true if the policy engine has rules loaded from a file.
    pub fn is_enabled(&self) -> bool {
        let state = self.state.read().expect("policy state lock poisoned");
        state.from_file
    }

    /// Verify SIEM endpoint reachability at startup.
    /// Delegates to the audit writer; returns Ok(()) if no writer is configured.
    pub fn verify_siem_reachability(&self) -> Result<(), String> {
        match &self.audit_writer {
            Some(w) => w.lock().expect("audit writer lock poisoned").verify_siem_reachability(),
            None => Ok(()),
        }
    }

    /// Force-reload the policy file. Used in tests to avoid waiting for
    /// the 30s poll interval.
    pub fn reload(&self) {
        if let Some(ref path) = self.policy_path {
            let new_state = load_policy_file(path);
            if let Ok(mut guard) = self.state.write() {
                *guard = new_state;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stub_always_allows() {
        let handle = BulwarkHandle::new_stub();

        let read_request = PolicyRequest {
            access_type: AccessType::Read,
            fact_id: None,
            agent_id: None,
            operation: "query".to_string(),
            domain_tags: vec![],
            fact_types: vec![],
        };
        assert_eq!(handle.check(&read_request), PolicyDecision::Allow);

        let write_request = PolicyRequest {
            access_type: AccessType::Write,
            fact_id: Some("fact-123".to_string()),
            agent_id: Some("agent-abc".to_string()),
            operation: "curate".to_string(),
            domain_tags: vec![],
            fact_types: vec![],
        };
        assert_eq!(handle.check(&write_request), PolicyDecision::Allow);
    }

    #[test]
    fn test_stub_audit_is_noop() {
        let handle = BulwarkHandle::new_stub();

        let req = PolicyRequest {
            access_type: AccessType::Write,
            fact_id: None,
            agent_id: None,
            operation: "curate".to_string(),
            domain_tags: vec![],
            fact_types: vec![],
        };
        let decision = PolicyDecision::Allow;
        for _ in 0..100 {
            handle.audit(&req, &decision, 0);
        }
    }

    #[test]
    fn test_stub_not_enabled() {
        let handle = BulwarkHandle::new_stub();
        assert!(!handle.is_enabled());
    }

    #[test]
    fn test_denying_handle() {
        let handle = BulwarkHandle::new_denying();
        assert!(!handle.is_enabled()); // deny_all() has from_file=false

        let req = PolicyRequest {
            access_type: AccessType::Read,
            fact_id: None,
            agent_id: None,
            operation: "query".to_string(),
            domain_tags: vec![],
            fact_types: vec![],
        };
        assert!(matches!(handle.check(&req), PolicyDecision::Deny { .. }));
    }

    #[test]
    fn test_from_config_missing_file() {
        let handle = BulwarkHandle::new_from_config(
            PathBuf::from("/nonexistent/bulwark.toml"),
            None,
            &AuditConfig::default(),
        );
        // Missing file → allow-all
        let req = PolicyRequest {
            access_type: AccessType::Write,
            fact_id: None,
            agent_id: None,
            operation: "compile".to_string(),
            domain_tags: vec![],
            fact_types: vec![],
        };
        assert_eq!(handle.check(&req), PolicyDecision::Allow);
    }

    #[test]
    fn test_from_config_valid_file() {
        let tmp = tempfile::tempdir().unwrap();
        let policy_path = tmp.path().join("bulwark.toml");
        std::fs::write(
            &policy_path,
            r#"
[[rules]]
name = "allow-read"
effect = "allow"
access_type = "read"

[[rules]]
name = "deny-all"
effect = "deny"
reason = "restricted"
"#,
        )
        .unwrap();

        let handle = BulwarkHandle::new_from_config(policy_path, None, &AuditConfig::default());
        assert!(handle.is_enabled());

        let read_req = PolicyRequest {
            access_type: AccessType::Read,
            fact_id: None,
            agent_id: None,
            operation: "query".to_string(),
            domain_tags: vec![],
            fact_types: vec![],
        };
        assert_eq!(handle.check(&read_req), PolicyDecision::Allow);

        let write_req = PolicyRequest {
            access_type: AccessType::Write,
            fact_id: None,
            agent_id: None,
            operation: "compile".to_string(),
            domain_tags: vec![],
            fact_types: vec![],
        };
        assert!(matches!(handle.check(&write_req), PolicyDecision::Deny { .. }));
    }

    #[test]
    fn test_reload() {
        let tmp = tempfile::tempdir().unwrap();
        let policy_path = tmp.path().join("bulwark.toml");

        // Start with allow-all
        std::fs::write(
            &policy_path,
            r#"
[[rules]]
name = "allow-all"
effect = "allow"
"#,
        )
        .unwrap();

        let handle = BulwarkHandle::new_from_config(policy_path.clone(), None, &AuditConfig::default());
        let req = PolicyRequest {
            access_type: AccessType::Write,
            fact_id: None,
            agent_id: None,
            operation: "compile".to_string(),
            domain_tags: vec![],
            fact_types: vec![],
        };
        assert_eq!(handle.check(&req), PolicyDecision::Allow);

        // Change to deny-all
        std::fs::write(
            &policy_path,
            r#"
[[rules]]
name = "deny-all"
effect = "deny"
reason = "locked down"
"#,
        )
        .unwrap();

        handle.reload();
        assert!(matches!(handle.check(&req), PolicyDecision::Deny { .. }));
    }

    /// SIGHUP triggers the background thread to reload the policy file
    /// within a few seconds (1s tick granularity).
    #[cfg(unix)]
    #[test]
    fn sighup_triggers_reload() {
        let tmp = tempfile::tempdir().unwrap();
        let policy_path = tmp.path().join("bulwark.toml");

        // Start with allow-all
        std::fs::write(
            &policy_path,
            r#"
[[rules]]
name = "allow-all"
effect = "allow"
"#,
        )
        .unwrap();

        let handle = BulwarkHandle::new_from_config(policy_path.clone(), None, &AuditConfig::default());

        // Give the background thread time to start and read initial file content.
        // Without this, the thread may read last_content after the overwrite below,
        // causing it to see no diff when SIGHUP triggers a reload check.
        thread::sleep(Duration::from_millis(100));

        let req = PolicyRequest {
            access_type: AccessType::Write,
            fact_id: None,
            agent_id: None,
            operation: "compile".to_string(),
            domain_tags: vec![],
            fact_types: vec![],
        };
        assert_eq!(handle.check(&req), PolicyDecision::Allow);

        // Overwrite with deny-all
        std::fs::write(
            &policy_path,
            r#"
[[rules]]
name = "deny-all"
effect = "deny"
reason = "sighup test"
"#,
        )
        .unwrap();

        // Poll until the background thread picks up the signal.
        // Re-send SIGHUP each iteration to handle the case where the flag
        // was consumed by the thread before it had a chance to read the
        // updated file (e.g. concurrent tests racing on tick boundaries).
        let mut reloaded = false;
        for _ in 0..10 {
            unsafe {
                libc::kill(libc::getpid(), libc::SIGHUP);
            }
            thread::sleep(Duration::from_secs(1));
            if matches!(handle.check(&req), PolicyDecision::Deny { .. }) {
                reloaded = true;
                break;
            }
        }
        assert!(reloaded, "policy should have reloaded to deny-all after SIGHUP");
    }
}
