pub mod audit;
pub mod policy;
pub mod rules;

pub use audit::{AuditEvent, AuditOutcome, AuditWriter, ChainError, verify_audit_chain};
pub use policy::{AccessType, PolicyDecision, PolicyRequest};
pub use rules::{evaluate_policy, load_policy_file, PolicyFile, PolicyRule, PolicyState};

use std::path::PathBuf;
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
    pub fn new_from_config(policy_path: PathBuf, audit_dir: Option<PathBuf>) -> Self {
        let initial_state = load_policy_file(&policy_path);
        let state = Arc::new(RwLock::new(initial_state));

        // Spawn hot-reload thread
        let reload_state = Arc::clone(&state);
        let reload_path = policy_path.clone();
        thread::spawn(move || {
            let mut last_content = std::fs::read_to_string(&reload_path).ok();
            loop {
                thread::sleep(Duration::from_secs(30));
                let current_content = std::fs::read_to_string(&reload_path).ok();
                if current_content != last_content {
                    let new_state = load_policy_file(&reload_path);
                    if let Ok(mut guard) = reload_state.write() {
                        *guard = new_state;
                    }
                    last_content = current_content;
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
            Some(Arc::new(Mutex::new(AuditWriter::new(log_path))))
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
        };
        assert_eq!(handle.check(&read_request), PolicyDecision::Allow);

        let write_request = PolicyRequest {
            access_type: AccessType::Write,
            fact_id: Some("fact-123".to_string()),
            agent_id: Some("agent-abc".to_string()),
            operation: "curate".to_string(),
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
        };
        assert!(matches!(handle.check(&req), PolicyDecision::Deny { .. }));
    }

    #[test]
    fn test_from_config_missing_file() {
        let handle = BulwarkHandle::new_from_config(
            PathBuf::from("/nonexistent/bulwark.toml"),
            None,
        );
        // Missing file → allow-all
        let req = PolicyRequest {
            access_type: AccessType::Write,
            fact_id: None,
            agent_id: None,
            operation: "compile".to_string(),
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

        let handle = BulwarkHandle::new_from_config(policy_path, None);
        assert!(handle.is_enabled());

        let read_req = PolicyRequest {
            access_type: AccessType::Read,
            fact_id: None,
            agent_id: None,
            operation: "query".to_string(),
        };
        assert_eq!(handle.check(&read_req), PolicyDecision::Allow);

        let write_req = PolicyRequest {
            access_type: AccessType::Write,
            fact_id: None,
            agent_id: None,
            operation: "compile".to_string(),
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

        let handle = BulwarkHandle::new_from_config(policy_path.clone(), None);
        let req = PolicyRequest {
            access_type: AccessType::Write,
            fact_id: None,
            agent_id: None,
            operation: "compile".to_string(),
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
}
