pub mod audit;
pub mod policy;

pub use audit::{AuditEvent, AuditOutcome};
pub use policy::{AccessType, PolicyDecision, PolicyRequest};

#[derive(Debug, Clone)]
pub struct BulwarkHandle {
    enabled: bool,
}

impl BulwarkHandle {
    /// Create a no-op stub handle. All requests are allowed.
    /// Phase 4 replaces this with a real policy engine constructor.
    pub fn new_stub() -> Self {
        BulwarkHandle { enabled: false }
    }

    /// Create a deny-all handle for testing policy denial paths.
    /// Used by downstream crates in their test suites.
    pub fn new_denying() -> Self {
        BulwarkHandle { enabled: true }
    }

    /// Evaluate a policy request.
    /// Stub returns Allow when not enabled, Deny when enabled (test-only).
    /// Phase 4 evaluates against real policy rules.
    pub fn check(&self, _request: &PolicyRequest) -> PolicyDecision {
        if self.enabled {
            return PolicyDecision::Deny {
                reason: "Bulwark enforcement active (stub deny)".to_string(),
            };
        }
        PolicyDecision::Allow
    }

    /// Record an audit event.
    /// Stub is a no-op.
    /// Phase 4 writes to the audit log.
    pub fn audit(&self, event: AuditEvent) {
        let _ = event;
    }

    /// Returns true if Bulwark enforcement is active.
    /// Stub always returns false.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

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

        for _ in 0..100 {
            let event = AuditEvent {
                request: PolicyRequest {
                    access_type: AccessType::Write,
                    fact_id: None,
                    agent_id: None,
                    operation: "curate".to_string(),
                },
                decision: PolicyDecision::Allow,
                outcome: AuditOutcome::Success,
                timestamp: Utc::now(),
            };
            handle.audit(event);
        }
    }

    #[test]
    fn test_stub_not_enabled() {
        let handle = BulwarkHandle::new_stub();
        assert!(!handle.is_enabled());
    }
}
