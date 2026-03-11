use chrono::{DateTime, Utc};

use crate::policy::{PolicyDecision, PolicyRequest};

#[derive(Debug, Clone)]
pub enum AuditOutcome {
    Success,
    Failure { reason: String },
}

#[derive(Debug, Clone)]
pub struct AuditEvent {
    pub request: PolicyRequest,
    pub decision: PolicyDecision,
    pub outcome: AuditOutcome,
    pub timestamp: DateTime<Utc>,
}
