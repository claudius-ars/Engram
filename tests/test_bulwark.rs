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
fn test_bulwark_denying_denies_all() {
    let bulwark = BulwarkHandle::new_denying();
    // new_denying() is a synthetic deny-all, not from a file
    assert!(!bulwark.is_enabled());
    let req = PolicyRequest {
        access_type: AccessType::Read,
        fact_id: None,
        agent_id: None,
        operation: "query".to_string(),
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
        };
        let decision = PolicyDecision::Allow;
        bulwark.audit(&req, &decision, 0); // should not panic
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
    );
    assert!(!handle.is_enabled());
    for at in [AccessType::Read, AccessType::Write, AccessType::LlmCall] {
        let req = PolicyRequest {
            access_type: at,
            fact_id: None,
            agent_id: None,
            operation: "test".to_string(),
        };
        assert_eq!(handle.check(&req), PolicyDecision::Allow);
    }
}

#[test]
fn test_policy_from_config_invalid_toml_denies_all() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("bulwark.toml");
    std::fs::write(&path, "this is {{not valid toml").unwrap();

    let handle = BulwarkHandle::new_from_config(path, None);
    for at in [AccessType::Read, AccessType::Write, AccessType::LlmCall] {
        let req = PolicyRequest {
            access_type: at,
            fact_id: None,
            agent_id: None,
            operation: "test".to_string(),
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

    let handle = BulwarkHandle::new_from_config(path, None);
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

    let llm_req = PolicyRequest {
        access_type: AccessType::LlmCall,
        fact_id: None,
        agent_id: None,
        operation: "tier3".to_string(),
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

    let handle = BulwarkHandle::new_from_config(path, None);

    let trusted_req = PolicyRequest {
        access_type: AccessType::Write,
        fact_id: None,
        agent_id: Some("trusted-agent".to_string()),
        operation: "compile".to_string(),
    };
    assert_eq!(handle.check(&trusted_req), PolicyDecision::Allow);

    let untrusted_req = PolicyRequest {
        access_type: AccessType::Write,
        fact_id: None,
        agent_id: Some("untrusted-bot".to_string()),
        operation: "compile".to_string(),
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

    let handle = BulwarkHandle::new_from_config(path, None);
    let req = PolicyRequest {
        access_type: AccessType::LlmCall,
        fact_id: None,
        agent_id: None,
        operation: "tier3".to_string(),
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

    let handle = BulwarkHandle::new_from_config(path.clone(), None);
    let req = PolicyRequest {
        access_type: AccessType::Write,
        fact_id: None,
        agent_id: None,
        operation: "compile".to_string(),
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
    };
    assert_eq!(handle.check(&read_req), PolicyDecision::Allow);
}

#[test]
fn test_policy_empty_rules_default_deny() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("bulwark.toml");
    std::fs::write(&path, "# empty policy\n").unwrap();

    let handle = BulwarkHandle::new_from_config(path, None);
    let req = PolicyRequest {
        access_type: AccessType::Write,
        fact_id: None,
        agent_id: None,
        operation: "compile".to_string(),
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

    let handle = BulwarkHandle::new_from_config(path, None);
    let req = PolicyRequest {
        access_type: AccessType::Read,
        fact_id: None,
        agent_id: Some("other-agent".to_string()),
        operation: "query".to_string(),
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

    let bulwark = BulwarkHandle::new_from_config(policy_path, None);
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

    let bulwark = BulwarkHandle::new_from_config(policy_path, None);
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
// Stub handle audit: no file created
// ============================================================

#[test]
fn audit_noop_for_stub_handle() {
    let tmp = tempfile::tempdir().unwrap();
    let audit_dir = tmp.path().join("audit");

    let handle = BulwarkHandle::new_stub();
    let req = PolicyRequest {
        access_type: AccessType::Read,
        fact_id: None,
        agent_id: None,
        operation: "query".to_string(),
    };
    let decision = PolicyDecision::Allow;
    handle.audit(&req, &decision, 0);

    assert!(
        !audit_dir.exists(),
        "stub handle should not create audit directory"
    );
}
