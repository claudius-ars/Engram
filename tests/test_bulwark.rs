use engram_core::AuditConfig;
#[allow(dead_code)]
mod common;

use std::path::PathBuf;

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
        domain_tags: vec![],
        fact_types: vec![],
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
        domain_tags: vec![],
        fact_types: vec![],
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
        domain_tags: vec![],
        fact_types: vec![],
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
        domain_tags: vec![],
        fact_types: vec![],
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
        domain_tags: vec![],
        fact_types: vec![],
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
        domain_tags: vec![],
        fact_types: vec![],
    };
    let decision = bulwark.check(&request);
    assert!(matches!(decision, PolicyDecision::Deny { .. }));
}

#[test]
fn test_bulwark_denying_denies_all() {
    let bulwark = BulwarkHandle::new_denying();
    // new_denying() is a synthetic deny-all, not from a file
    assert!(!bulwark.is_enabled());
    let req = PolicyRequest {
        access_type: AccessType::Read,
        fact_id: None,
        agent_id: None,
        operation: "query".to_string(),
        domain_tags: vec![],
        fact_types: vec![],
    };
    assert!(matches!(bulwark.check(&req), PolicyDecision::Deny { .. }));
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
        let req = PolicyRequest {
            access_type,
            fact_id: None,
            agent_id: None,
            operation: "test".to_string(),
            domain_tags: vec![],
            fact_types: vec![],
        };
        let decision = PolicyDecision::Allow;
        bulwark.audit(&req, &decision, 0); // fixture: duration_ms=0 is intentional
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

// ============================================================
// File-backed policy engine tests
// ============================================================

#[test]
fn test_policy_from_config_missing_file_allows_all() {
    let handle = BulwarkHandle::new_from_config(
        PathBuf::from("/nonexistent/bulwark.toml"),
        None,
        &AuditConfig::default(),
    );
    assert!(!handle.is_enabled());
    for at in [AccessType::Read, AccessType::Write, AccessType::LlmCall] {
        let req = PolicyRequest {
            access_type: at,
            fact_id: None,
            agent_id: None,
            operation: "test".to_string(),
            domain_tags: vec![],
            fact_types: vec![],
        };
        assert_eq!(handle.check(&req), PolicyDecision::Allow);
    }
}

#[test]
fn test_policy_from_config_invalid_toml_denies_all() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("bulwark.toml");
    std::fs::write(&path, "this is {{not valid toml").unwrap();

    let handle = BulwarkHandle::new_from_config(path, None, &AuditConfig::default());
    for at in [AccessType::Read, AccessType::Write, AccessType::LlmCall] {
        let req = PolicyRequest {
            access_type: at,
            fact_id: None,
            agent_id: None,
            operation: "test".to_string(),
            domain_tags: vec![],
            fact_types: vec![],
        };
        assert!(matches!(handle.check(&req), PolicyDecision::Deny { .. }));
    }
}

#[test]
fn test_policy_first_match_allow_read_deny_rest() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("bulwark.toml");
    std::fs::write(
        &path,
        r#"
[[rules]]
name = "allow-read"
effect = "allow"
access_type = "read"

[[rules]]
name = "deny-everything"
effect = "deny"
reason = "restricted workspace"
"#,
    )
    .unwrap();

    let handle = BulwarkHandle::new_from_config(path, None, &AuditConfig::default());
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

    let llm_req = PolicyRequest {
        access_type: AccessType::LlmCall,
        fact_id: None,
        agent_id: None,
        operation: "tier3".to_string(),
        domain_tags: vec![],
        fact_types: vec![],
    };
    assert!(matches!(handle.check(&llm_req), PolicyDecision::Deny { .. }));
}

#[test]
fn test_policy_agent_specific_deny() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("bulwark.toml");
    std::fs::write(
        &path,
        r#"
[[rules]]
name = "block-untrusted"
effect = "deny"
agent = "untrusted-*"
reason = "untrusted agent"

[[rules]]
name = "allow-all"
effect = "allow"
"#,
    )
    .unwrap();

    let handle = BulwarkHandle::new_from_config(path, None, &AuditConfig::default());

    let trusted_req = PolicyRequest {
        access_type: AccessType::Write,
        fact_id: None,
        agent_id: Some("trusted-agent".to_string()),
        operation: "compile".to_string(),
        domain_tags: vec![],
        fact_types: vec![],
    };
    assert_eq!(handle.check(&trusted_req), PolicyDecision::Allow);

    let untrusted_req = PolicyRequest {
        access_type: AccessType::Write,
        fact_id: None,
        agent_id: Some("untrusted-bot".to_string()),
        operation: "compile".to_string(),
        domain_tags: vec![],
        fact_types: vec![],
    };
    assert!(matches!(
        handle.check(&untrusted_req),
        PolicyDecision::Deny { .. }
    ));
}

#[test]
fn test_policy_deny_has_rule_name() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("bulwark.toml");
    std::fs::write(
        &path,
        r#"
[[rules]]
name = "no-llm"
effect = "deny"
access_type = "llm_call"
reason = "LLM calls disabled"
"#,
    )
    .unwrap();

    let handle = BulwarkHandle::new_from_config(path, None, &AuditConfig::default());
    let req = PolicyRequest {
        access_type: AccessType::LlmCall,
        fact_id: None,
        agent_id: None,
        operation: "tier3".to_string(),
        domain_tags: vec![],
        fact_types: vec![],
    };
    match handle.check(&req) {
        PolicyDecision::Deny { reason, rule_name } => {
            assert_eq!(reason, "LLM calls disabled");
            assert_eq!(rule_name, Some("no-llm".to_string()));
        }
        PolicyDecision::Allow => panic!("expected deny"),
    }
}

#[test]
fn test_policy_hot_reload() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("bulwark.toml");
    std::fs::write(
        &path,
        r#"
[[rules]]
name = "allow-all"
effect = "allow"
"#,
    )
    .unwrap();

    let handle = BulwarkHandle::new_from_config(path.clone(), None, &AuditConfig::default());
    let req = PolicyRequest {
        access_type: AccessType::Write,
        fact_id: None,
        agent_id: None,
        operation: "compile".to_string(),
        domain_tags: vec![],
        fact_types: vec![],
    };
    assert_eq!(handle.check(&req), PolicyDecision::Allow);

    // Update policy file to deny writes
    std::fs::write(
        &path,
        r#"
[[rules]]
name = "deny-writes"
effect = "deny"
access_type = "write"
reason = "locked"

[[rules]]
name = "allow-rest"
effect = "allow"
"#,
    )
    .unwrap();

    // Force reload (don't wait 30s in test)
    handle.reload();

    assert!(matches!(handle.check(&req), PolicyDecision::Deny { .. }));

    // Reads should still be allowed
    let read_req = PolicyRequest {
        access_type: AccessType::Read,
        fact_id: None,
        agent_id: None,
        operation: "query".to_string(),
        domain_tags: vec![],
        fact_types: vec![],
    };
    assert_eq!(handle.check(&read_req), PolicyDecision::Allow);
}

#[test]
fn test_policy_empty_rules_default_deny() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("bulwark.toml");
    std::fs::write(&path, "# empty policy\n").unwrap();

    let handle = BulwarkHandle::new_from_config(path, None, &AuditConfig::default());
    let req = PolicyRequest {
        access_type: AccessType::Write,
        fact_id: None,
        agent_id: None,
        operation: "compile".to_string(),
        domain_tags: vec![],
        fact_types: vec![],
    };
    assert!(matches!(handle.check(&req), PolicyDecision::Deny { .. }));
}

#[test]
fn test_policy_no_match_default_deny() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("bulwark.toml");
    // Rule only matches agent "special-agent" — any other request won't match
    std::fs::write(
        &path,
        r#"
[[rules]]
name = "allow-special"
effect = "allow"
agent = "special-agent"
"#,
    )
    .unwrap();

    let handle = BulwarkHandle::new_from_config(path, None, &AuditConfig::default());
    let req = PolicyRequest {
        access_type: AccessType::Read,
        fact_id: None,
        agent_id: Some("other-agent".to_string()),
        operation: "query".to_string(),
        domain_tags: vec![],
        fact_types: vec![],
    };
    match handle.check(&req) {
        PolicyDecision::Deny { rule_name, .. } => {
            assert_eq!(rule_name, None, "no rule matched — rule_name should be None");
        }
        PolicyDecision::Allow => panic!("expected default deny when no rule matches"),
    }
}

#[test]
fn test_policy_compile_pipeline_with_file_deny() {
    let tmp = common::temp_workspace();
    common::write_fact(
        tmp.path(),
        "fact.md",
        &common::durable_fact("Test Fact", "Some content for testing."),
    );

    let policy_path = tmp.path().join("bulwark.toml");
    std::fs::write(
        &policy_path,
        r#"
[[rules]]
name = "deny-all"
effect = "deny"
reason = "workspace locked"
"#,
    )
    .unwrap();

    let bulwark = BulwarkHandle::new_from_config(policy_path, None, &AuditConfig::default());
    let result = engram_compiler::compile_context_tree(tmp.path(), true, &bulwark);
    assert!(
        result.index_error.is_some(),
        "compile should fail when file policy denies"
    );
}

#[test]
fn test_policy_query_pipeline_with_file_deny() {
    let tmp = common::temp_workspace();
    common::write_fact(
        tmp.path(),
        "fact.md",
        &common::durable_fact("Test Fact", "Some content for testing."),
    );
    common::compile_clean(tmp.path());

    let policy_path = tmp.path().join("bulwark.toml");
    std::fs::write(
        &policy_path,
        r#"
[[rules]]
name = "deny-reads"
effect = "deny"
access_type = "read"
reason = "read access revoked"
"#,
    )
    .unwrap();

    let bulwark = BulwarkHandle::new_from_config(policy_path, None, &AuditConfig::default());
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

    assert!(result.is_err(), "query should fail when file policy denies reads");
}

// ============================================================
// Phase 6: Full PolicyRule field coverage tests
// ============================================================

fn make_policy_state(toml: &str) -> engram_bulwark::BulwarkHandle {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("bulwark.toml");
    std::fs::write(&path, toml).unwrap();
    // Leak the tempdir so the file persists for the test
    let handle = BulwarkHandle::new_from_config(path, None, &AuditConfig::default());
    std::mem::forget(tmp);
    handle
}

#[test]
fn operations_filter_allows() {
    let handle = make_policy_state(r#"
[[rules]]
name = "allow-query-only"
effect = "allow"
operations = ["query"]

[[rules]]
name = "default-deny"
effect = "deny"
"#);

    let req = PolicyRequest {
        access_type: AccessType::Read,
        fact_id: None,
        agent_id: None,
        operation: "query".to_string(),
        domain_tags: vec![],
        fact_types: vec![],
    };
    assert_eq!(handle.check(&req), PolicyDecision::Allow);
}

#[test]
fn operations_filter_denies() {
    let handle = make_policy_state(r#"
[[rules]]
name = "allow-query-only"
effect = "allow"
operations = ["query"]

[[rules]]
name = "default-deny"
effect = "deny"
reason = "not query"
"#);

    let req = PolicyRequest {
        access_type: AccessType::Write,
        fact_id: None,
        agent_id: None,
        operation: "compile".to_string(),
        domain_tags: vec![],
        fact_types: vec![],
    };
    assert!(matches!(handle.check(&req), PolicyDecision::Deny { .. }));
}

#[test]
fn domain_tags_allow_all_match() {
    let handle = make_policy_state(r#"
[[rules]]
name = "allow-iso16530"
effect = "allow"
domain_tags_allow = ["iso16530:*"]

[[rules]]
name = "default-deny"
effect = "deny"
"#);

    let req = PolicyRequest {
        access_type: AccessType::Read,
        fact_id: None,
        agent_id: None,
        operation: "query".to_string(),
        domain_tags: vec!["iso16530:wellbore".to_string(), "iso16530:pressure".to_string()],
        fact_types: vec![],
    };
    assert_eq!(handle.check(&req), PolicyDecision::Allow);
}

#[test]
fn domain_tags_allow_partial_fail() {
    let handle = make_policy_state(r#"
[[rules]]
name = "allow-iso16530"
effect = "allow"
domain_tags_allow = ["iso16530:*"]

[[rules]]
name = "default-deny"
effect = "deny"
reason = "tag mismatch"
"#);

    // Second tag (osdu:wellbore) does not match iso16530:*
    let req = PolicyRequest {
        access_type: AccessType::Read,
        fact_id: None,
        agent_id: None,
        operation: "query".to_string(),
        domain_tags: vec!["iso16530:wellbore".to_string(), "osdu:wellbore".to_string()],
        fact_types: vec![],
    };
    assert!(matches!(handle.check(&req), PolicyDecision::Deny { .. }));
}

#[test]
fn domain_tags_deny_exclusion() {
    let handle = make_policy_state(r#"
[[rules]]
name = "allow-with-exclusion"
effect = "allow"
domain_tags_deny = ["internal:hr"]

[[rules]]
name = "default-deny"
effect = "deny"
reason = "excluded"
"#);

    // Request with excluded tag falls through to default deny
    let req_excluded = PolicyRequest {
        access_type: AccessType::Read,
        fact_id: None,
        agent_id: None,
        operation: "query".to_string(),
        domain_tags: vec!["internal:hr".to_string()],
        fact_types: vec![],
    };
    assert!(matches!(handle.check(&req_excluded), PolicyDecision::Deny { .. }));

    // Request without excluded tag matches the allow rule
    let req_ok = PolicyRequest {
        access_type: AccessType::Read,
        fact_id: None,
        agent_id: None,
        operation: "query".to_string(),
        domain_tags: vec!["iso16530:wellbore".to_string()],
        fact_types: vec![],
    };
    assert_eq!(handle.check(&req_ok), PolicyDecision::Allow);
}

#[test]
fn domain_tags_allow_empty_request_vacuous() {
    let handle = make_policy_state(r#"
[[rules]]
name = "allow-iso16530"
effect = "allow"
domain_tags_allow = ["iso16530:*"]

[[rules]]
name = "default-deny"
effect = "deny"
"#);

    // Empty domain_tags passes vacuously (ALL of empty set is true)
    let req = PolicyRequest {
        access_type: AccessType::Read,
        fact_id: None,
        agent_id: None,
        operation: "query".to_string(),
        domain_tags: vec![],
        fact_types: vec![],
    };
    assert_eq!(handle.check(&req), PolicyDecision::Allow);
}

#[test]
fn fact_types_filter() {
    let handle = make_policy_state(r#"
[[rules]]
name = "allow-durable"
effect = "allow"
fact_types = ["durable"]

[[rules]]
name = "default-deny"
effect = "deny"
reason = "fact type not allowed"
"#);

    // Request with durable fact type matches
    let req_ok = PolicyRequest {
        access_type: AccessType::Read,
        fact_id: None,
        agent_id: None,
        operation: "query".to_string(),
        domain_tags: vec![],
        fact_types: vec!["durable".to_string()],
    };
    assert_eq!(handle.check(&req_ok), PolicyDecision::Allow);

    // Request with event fact type falls to default deny
    let req_deny = PolicyRequest {
        access_type: AccessType::Read,
        fact_id: None,
        agent_id: None,
        operation: "query".to_string(),
        domain_tags: vec![],
        fact_types: vec!["event".to_string()],
    };
    assert!(matches!(handle.check(&req_deny), PolicyDecision::Deny { .. }));
}

// ============================================================
// Phase 7: fact_types population at curate call sites
// ============================================================

#[test]
fn test_fact_type_policy_allows_durable() {
    let handle = make_policy_state(r#"
[[rules]]
name = "allow-durable-curate"
effect = "allow"
fact_types = ["durable"]

[[rules]]
name = "default-deny"
effect = "deny"
reason = "fact type not allowed"
"#);

    let req = PolicyRequest {
        access_type: AccessType::Write,
        fact_id: None,
        agent_id: None,
        operation: "curate".to_string(),
        domain_tags: vec![],
        fact_types: vec!["durable".to_string()],
    };
    assert_eq!(handle.check(&req), PolicyDecision::Allow);
}

#[test]
fn test_fact_type_policy_denies_event() {
    let handle = make_policy_state(r#"
[[rules]]
name = "allow-durable-only"
effect = "allow"
fact_types = ["durable"]

[[rules]]
name = "default-deny"
effect = "deny"
reason = "only durable facts allowed"
"#);

    let req = PolicyRequest {
        access_type: AccessType::Write,
        fact_id: None,
        agent_id: None,
        operation: "curate".to_string(),
        domain_tags: vec![],
        fact_types: vec!["event".to_string()],
    };
    assert!(matches!(handle.check(&req), PolicyDecision::Deny { .. }));
}

#[test]
fn test_fact_type_empty_rule_allows_all() {
    let handle = make_policy_state(r#"
[[rules]]
name = "allow-all-types"
effect = "allow"
fact_types = []

[[rules]]
name = "default-deny"
effect = "deny"
"#);

    let req = PolicyRequest {
        access_type: AccessType::Write,
        fact_id: None,
        agent_id: None,
        operation: "curate".to_string(),
        domain_tags: vec![],
        fact_types: vec!["event".to_string()],
    };
    assert_eq!(handle.check(&req), PolicyDecision::Allow);
}

