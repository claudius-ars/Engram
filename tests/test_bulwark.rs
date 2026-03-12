#[allow(dead_code)]
mod common;

use engram_bulwark::{AccessType, BulwarkHandle, PolicyDecision, PolicyRequest};

// ============================================================
// Stub handle: allow-all, including LlmCall
// ============================================================

#[test]
fn test_bulwark_stub_allows_read() {
    let bulwark = BulwarkHandle::new_stub();
    let request = PolicyRequest {
        access_type: AccessType::Read,
        fact_id: None,
        agent_id: None,
        operation: "query".to_string(),
    };
    assert_eq!(bulwark.check(&request), PolicyDecision::Allow);
}

#[test]
fn test_bulwark_stub_allows_write() {
    let bulwark = BulwarkHandle::new_stub();
    let request = PolicyRequest {
        access_type: AccessType::Write,
        fact_id: Some("fact-123".to_string()),
        agent_id: Some("agent-abc".to_string()),
        operation: "compile".to_string(),
    };
    assert_eq!(bulwark.check(&request), PolicyDecision::Allow);
}

#[test]
fn test_bulwark_stub_allows_llm_call() {
    let bulwark = BulwarkHandle::new_stub();
    let request = PolicyRequest {
        access_type: AccessType::LlmCall,
        fact_id: None,
        agent_id: Some("any-agent".to_string()),
        operation: "tier3_llm_synthesis".to_string(),
    };
    assert_eq!(bulwark.check(&request), PolicyDecision::Allow);
}

#[test]
fn test_bulwark_stub_not_enabled() {
    let bulwark = BulwarkHandle::new_stub();
    assert!(!bulwark.is_enabled());
}

// ============================================================
// Denying handle: deny-all, including LlmCall
// ============================================================

#[test]
fn test_bulwark_denying_blocks_read() {
    let bulwark = BulwarkHandle::new_denying();
    let request = PolicyRequest {
        access_type: AccessType::Read,
        fact_id: None,
        agent_id: None,
        operation: "query".to_string(),
    };
    let decision = bulwark.check(&request);
    assert!(matches!(decision, PolicyDecision::Deny { .. }));
}

#[test]
fn test_bulwark_denying_blocks_write() {
    let bulwark = BulwarkHandle::new_denying();
    let request = PolicyRequest {
        access_type: AccessType::Write,
        fact_id: Some("fact-1".to_string()),
        agent_id: Some("agent-1".to_string()),
        operation: "compile".to_string(),
    };
    let decision = bulwark.check(&request);
    assert!(matches!(decision, PolicyDecision::Deny { .. }));
}

#[test]
fn test_bulwark_denying_blocks_llm_call() {
    let bulwark = BulwarkHandle::new_denying();
    let request = PolicyRequest {
        access_type: AccessType::LlmCall,
        fact_id: None,
        agent_id: Some("any-agent".to_string()),
        operation: "tier3_llm_synthesis".to_string(),
    };
    let decision = bulwark.check(&request);
    assert!(matches!(decision, PolicyDecision::Deny { .. }));
}

#[test]
fn test_bulwark_denying_is_enabled() {
    let bulwark = BulwarkHandle::new_denying();
    assert!(bulwark.is_enabled());
}

// ============================================================
// Policy integration: deny blocks compile and query pipelines
// ============================================================

#[test]
fn test_bulwark_deny_blocks_compile_pipeline() {
    let tmp = common::temp_workspace();
    common::write_fact(
        tmp.path(),
        "fact.md",
        &common::durable_fact("Test Fact", "Some content for testing."),
    );

    let bulwark = BulwarkHandle::new_denying();
    let result = engram_compiler::compile_context_tree(tmp.path(), true, &bulwark);

    assert!(
        result.index_error.is_some(),
        "compile should fail when Bulwark denies"
    );
}

#[test]
fn test_bulwark_deny_blocks_query_pipeline() {
    let tmp = common::temp_workspace();
    common::write_fact(
        tmp.path(),
        "fact.md",
        &common::durable_fact("Test Fact", "Some content for testing."),
    );
    common::compile_clean(tmp.path());

    let bulwark = BulwarkHandle::new_denying();
    let config = engram_core::WorkspaceConfig {
        score_threshold: 0.0,
        score_gap: 0.0,
        ..engram_core::WorkspaceConfig::default()
    };
    let mut cache = engram_query::ExactCache::new(60);
    let mut fuzzy = engram_query::FuzzyCache::new(100);

    let result = engram_query::query(
        tmp.path(),
        "test",
        engram_query::QueryOptions::default(),
        &mut cache,
        &mut fuzzy,
        &bulwark,
        &config,
    );

    assert!(result.is_err(), "query should fail when Bulwark denies");
}

// ============================================================
// Audit events: stub handles audit without error
// ============================================================

#[test]
fn test_bulwark_audit_no_panic() {
    let bulwark = BulwarkHandle::new_stub();
    for access_type in [AccessType::Read, AccessType::Write, AccessType::LlmCall] {
        let event = engram_bulwark::AuditEvent {
            request: PolicyRequest {
                access_type,
                fact_id: None,
                agent_id: None,
                operation: "test".to_string(),
            },
            decision: PolicyDecision::Allow,
            outcome: engram_bulwark::AuditOutcome::Success,
            timestamp: chrono::Utc::now(),
        };
        bulwark.audit(event); // should not panic
    }
}

// ============================================================
// Access type coverage: all three variants are distinct
// ============================================================

#[test]
fn test_access_type_enum_distinct() {
    assert_ne!(AccessType::Read, AccessType::Write);
    assert_ne!(AccessType::Read, AccessType::LlmCall);
    assert_ne!(AccessType::Write, AccessType::LlmCall);
}
