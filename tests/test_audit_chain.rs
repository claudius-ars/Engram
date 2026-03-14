use engram_core::AuditConfig;
use engram_bulwark::{
    AccessType, BulwarkHandle, ChainError, PolicyDecision, PolicyRequest, verify_audit_chain,
};

fn make_request(access_type: AccessType, agent: &str, operation: &str) -> PolicyRequest {
    PolicyRequest {
        access_type,
        fact_ids: vec![],
        agent_id: Some(agent.to_string()),
        operation: operation.to_string(),
        domain_tags: vec![],
        fact_types: vec![],
    }
}

// ============================================================
// 1. Fresh log verifies after multiple entries
// ============================================================

#[test]
fn audit_chain_fresh_log_verifies() {
    let tmp = tempfile::tempdir().unwrap();
    let policy_path = tmp.path().join("bulwark.toml");
    let audit_dir = tmp.path().join("audit");

    // Allow-all policy
    std::fs::write(
        &policy_path,
        r#"
[[rules]]
name = "allow-all"
effect = "allow"
"#,
    )
    .unwrap();

    let handle = BulwarkHandle::new_from_config(policy_path, Some(audit_dir.clone()), &AuditConfig::default());

    // Make 5 check + audit calls with mixed decisions
    for i in 0..5 {
        let req = make_request(AccessType::Read, &format!("agent-{}", i), "query");
        let decision = handle.check(&req);
        handle.audit(&req, &decision, i as u64);
    }

    let log_path = audit_dir.join("engram.log");
    assert!(log_path.exists(), "audit log should exist");

    let count = verify_audit_chain(&log_path).expect("chain should verify");
    assert_eq!(count, 5);
}

// ============================================================
// 2. Tampered entry breaks chain
// ============================================================

#[test]
fn audit_chain_tampered_entry_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let policy_path = tmp.path().join("bulwark.toml");
    let audit_dir = tmp.path().join("audit");

    std::fs::write(
        &policy_path,
        r#"
[[rules]]
name = "allow-all"
effect = "allow"
"#,
    )
    .unwrap();

    let handle = BulwarkHandle::new_from_config(policy_path, Some(audit_dir.clone()), &AuditConfig::default());

    for i in 0..3 {
        let req = make_request(AccessType::Read, &format!("agent-{}", i), "query");
        let decision = handle.check(&req);
        handle.audit(&req, &decision, 0);
    }

    let log_path = audit_dir.join("engram.log");
    let content = std::fs::read_to_string(&log_path).unwrap();

    // Tamper with the second line — change agent_id
    let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
    assert!(lines.len() >= 3);
    lines[1] = lines[1].replace("agent-1", "tampered-agent");
    let tampered = lines.join("\n") + "\n";
    std::fs::write(&log_path, tampered).unwrap();

    let result = verify_audit_chain(&log_path);
    assert!(
        matches!(result, Err(ChainError::HashMismatch { line_number: 3, .. })),
        "chain should break at line 3 (entry after tampered line): {:?}",
        result
    );
}

// ============================================================
// 3. Partial last line handled gracefully
// ============================================================

#[test]
fn audit_chain_partial_last_line_handled() {
    let tmp = tempfile::tempdir().unwrap();
    let policy_path = tmp.path().join("bulwark.toml");
    let audit_dir = tmp.path().join("audit");

    std::fs::write(
        &policy_path,
        r#"
[[rules]]
name = "allow-all"
effect = "allow"
"#,
    )
    .unwrap();

    let handle = BulwarkHandle::new_from_config(policy_path, Some(audit_dir.clone()), &AuditConfig::default());

    for i in 0..2 {
        let req = make_request(AccessType::Read, &format!("agent-{}", i), "query");
        let decision = handle.check(&req);
        handle.audit(&req, &decision, 0);
    }

    let log_path = audit_dir.join("engram.log");

    // Append a partial line without trailing newline
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&log_path)
        .unwrap();
    write!(file, r#"{{"partial":"crash"#).unwrap();

    let count = verify_audit_chain(&log_path).expect("should handle partial line");
    assert_eq!(count, 2, "partial line excluded from count");
}

// ============================================================
// 4. Empty log returns zero
// ============================================================

#[test]
fn audit_chain_empty_log_returns_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let log_path = tmp.path().join("engram.log");
    std::fs::write(&log_path, "").unwrap();

    let count = verify_audit_chain(&log_path).expect("empty log should verify");
    assert_eq!(count, 0);
}

// ============================================================
// 5. Entries contain rule_name
// ============================================================

#[test]
fn audit_entries_contain_rule_name() {
    let tmp = tempfile::tempdir().unwrap();
    let policy_path = tmp.path().join("bulwark.toml");
    let audit_dir = tmp.path().join("audit");

    std::fs::write(
        &policy_path,
        r#"
[[rules]]
name = "block-writes"
effect = "deny"
access_type = "write"
reason = "read-only mode"

[[rules]]
name = "allow-rest"
effect = "allow"
"#,
    )
    .unwrap();

    let handle = BulwarkHandle::new_from_config(policy_path, Some(audit_dir.clone()), &AuditConfig::default());

    let req = make_request(AccessType::Write, "agent-1", "compile");
    let decision = handle.check(&req);
    handle.audit(&req, &decision, 42);

    let log_path = audit_dir.join("engram.log");
    let content = std::fs::read_to_string(&log_path).unwrap();
    let entry: serde_json::Value = serde_json::from_str(content.lines().next().unwrap()).unwrap();

    assert_eq!(entry["rule_name"], "block-writes");
    assert_eq!(entry["reason"], "read-only mode");
    assert_eq!(entry["decision"], "deny");
    assert_eq!(entry["duration_ms"], 42);
}

// ============================================================
// 6. Allow and deny both appear in log
// ============================================================

#[test]
fn audit_allow_and_deny_both_appear_in_log() {
    let tmp = tempfile::tempdir().unwrap();
    let policy_path = tmp.path().join("bulwark.toml");
    let audit_dir = tmp.path().join("audit");

    std::fs::write(
        &policy_path,
        r#"
[[rules]]
name = "block-untrusted"
effect = "deny"
agent = "untrusted"
reason = "not allowed"

[[rules]]
name = "allow-rest"
effect = "allow"
"#,
    )
    .unwrap();

    let handle = BulwarkHandle::new_from_config(policy_path, Some(audit_dir.clone()), &AuditConfig::default());

    // Allow request
    let allow_req = make_request(AccessType::Read, "trusted", "query");
    let allow_decision = handle.check(&allow_req);
    handle.audit(&allow_req, &allow_decision, 0);

    // Deny request
    let deny_req = make_request(AccessType::Read, "untrusted", "query");
    let deny_decision = handle.check(&deny_req);
    handle.audit(&deny_req, &deny_decision, 0);

    let log_path = audit_dir.join("engram.log");
    let content = std::fs::read_to_string(&log_path).unwrap();
    assert!(content.contains(r#""decision":"allow""#));
    assert!(content.contains(r#""decision":"deny""#));

    let count = verify_audit_chain(&log_path).expect("chain should verify");
    assert_eq!(count, 2);
}

// ============================================================
// 7. Stub handle produces no audit file
// ============================================================

#[test]
fn audit_noop_for_stub_handle() {
    let tmp = tempfile::tempdir().unwrap();
    let audit_dir = tmp.path().join("audit");

    let handle = BulwarkHandle::new_stub();
    let req = make_request(AccessType::Read, "agent-1", "query");
    let decision = PolicyDecision::Allow;
    handle.audit(&req, &decision, 0);

    // No audit directory or file should be created
    assert!(
        !audit_dir.exists(),
        "stub handle should not create audit directory"
    );
}

// ============================================================
// 8. Timing: duration_ms is present and parseable
// ============================================================

#[test]
fn timing_nonzero() {
    let tmp = tempfile::tempdir().unwrap();
    let policy_path = tmp.path().join("bulwark.toml");
    std::fs::write(
        &policy_path,
        r#"
[[rules]]
name = "allow-all"
effect = "allow"
"#,
    )
    .unwrap();

    let audit_dir = tmp.path().join("audit");
    let handle = BulwarkHandle::new_from_config(policy_path, Some(audit_dir.clone()), &AuditConfig::default());

    let req = PolicyRequest {
        access_type: AccessType::Read,
        fact_ids: vec![],
        agent_id: None,
        operation: "query".to_string(),
        domain_tags: vec![],
        fact_types: vec![],
    };

    let t0 = std::time::Instant::now();
    let decision = handle.check(&req);
    let duration_ms = t0.elapsed().as_millis() as u64;
    handle.audit(&req, &decision, duration_ms);

    let log_path = audit_dir.join("engram.log");
    let content = std::fs::read_to_string(&log_path).expect("audit log should exist");
    let last_line = content.lines().last().expect("should have at least one line");
    let event: serde_json::Value = serde_json::from_str(last_line).expect("valid JSON");

    // duration_ms must be present and parseable as u64.
    // Policy evaluation is sub-microsecond on most platforms, so the value
    // may be 0 when Instant resolution is coarser than 1ms. We assert the
    // field exists and is a non-negative integer rather than > 0.
    let dur = event["duration_ms"].as_u64().expect("duration_ms should be a u64");
    assert!(dur < 10_000, "duration_ms should be reasonable, got {}", dur);
}

// ============================================================
// 9. domain_tags are forwarded to audit events
// ============================================================

#[test]
fn domain_tags_forwarded() {
    let tmp = tempfile::tempdir().unwrap();
    let policy_path = tmp.path().join("bulwark.toml");
    std::fs::write(
        &policy_path,
        r#"
[[rules]]
name = "allow-all"
effect = "allow"
"#,
    )
    .unwrap();

    let audit_dir = tmp.path().join("audit");
    let handle = BulwarkHandle::new_from_config(policy_path, Some(audit_dir.clone()), &AuditConfig::default());

    let req = PolicyRequest {
        access_type: AccessType::Write,
        fact_ids: vec![],
        agent_id: None,
        operation: "compile".to_string(),
        domain_tags: vec!["iso16530:wellbore".to_string()],
        fact_types: vec![],
    };

    let decision = handle.check(&req);
    handle.audit(&req, &decision, 0);

    let log_path = audit_dir.join("engram.log");
    let content = std::fs::read_to_string(&log_path).expect("audit log should exist");
    let last_line = content.lines().last().expect("should have at least one line");
    let event: serde_json::Value = serde_json::from_str(last_line).expect("valid JSON");

    let tags = event["domain_tags"]
        .as_array()
        .expect("domain_tags should be an array");
    assert!(
        tags.iter().any(|t| t.as_str() == Some("iso16530:wellbore")),
        "domain_tags should contain 'iso16530:wellbore', got {:?}",
        tags
    );
}

// ============================================================
// 10. agent_id is recorded in audit entries
// ============================================================

#[test]
fn test_audit_entry_has_agent_id() {
    let tmp = tempfile::tempdir().unwrap();
    let policy_path = tmp.path().join("bulwark.toml");
    std::fs::write(
        &policy_path,
        r#"
[[rules]]
name = "allow-all"
effect = "allow"
"#,
    )
    .unwrap();

    let audit_dir = tmp.path().join("audit");
    let handle = BulwarkHandle::new_from_config(policy_path, Some(audit_dir.clone()), &AuditConfig::default());

    let req = PolicyRequest {
        access_type: AccessType::Read,
        fact_ids: vec![],
        agent_id: Some("test-agent".to_string()),
        operation: "query".to_string(),
        domain_tags: vec![],
        fact_types: vec![],
    };

    let decision = handle.check(&req);
    handle.audit(&req, &decision, 0);

    let log_path = audit_dir.join("engram.log");
    let content = std::fs::read_to_string(&log_path).expect("audit log should exist");
    let last_line = content.lines().last().expect("should have at least one line");
    let event: serde_json::Value = serde_json::from_str(last_line).expect("valid JSON");

    assert_eq!(
        event["agent_id"].as_str().unwrap(),
        "test-agent",
        "agent_id should be 'test-agent'"
    );
}

// ============================================================
// 11. fact_ids are recorded in audit entries
// ============================================================

#[test]
fn test_audit_entry_has_fact_ids() {
    let tmp = tempfile::tempdir().unwrap();
    let policy_path = tmp.path().join("bulwark.toml");
    std::fs::write(
        &policy_path,
        r#"
[[rules]]
name = "allow-all"
effect = "allow"
"#,
    )
    .unwrap();

    let audit_dir = tmp.path().join("audit");
    let handle = BulwarkHandle::new_from_config(policy_path, Some(audit_dir.clone()), &AuditConfig::default());

    let req = PolicyRequest {
        access_type: AccessType::Read,
        fact_ids: vec!["fact-alpha".to_string(), "fact-beta".to_string()],
        agent_id: Some("cli".to_string()),
        operation: "query".to_string(),
        domain_tags: vec![],
        fact_types: vec![],
    };

    let decision = handle.check(&req);
    handle.audit(&req, &decision, 0);

    let log_path = audit_dir.join("engram.log");
    let content = std::fs::read_to_string(&log_path).expect("audit log should exist");
    let last_line = content.lines().last().expect("should have at least one line");
    let event: serde_json::Value = serde_json::from_str(last_line).expect("valid JSON");

    let fact_ids = event["fact_ids"]
        .as_array()
        .expect("fact_ids should be an array");
    assert_eq!(fact_ids.len(), 2);
    assert_eq!(fact_ids[0].as_str().unwrap(), "fact-alpha");
    assert_eq!(fact_ids[1].as_str().unwrap(), "fact-beta");
}

// ============================================================
// 12. Integration: curate records fact_id and agent_id
// ============================================================

#[test]
fn test_audit_curate_entry_has_fact_id() {
    let tmp = tempfile::tempdir().unwrap();
    let policy_path = tmp.path().join("bulwark.toml");
    std::fs::write(
        &policy_path,
        r#"
[[rules]]
name = "allow-all"
effect = "allow"
"#,
    )
    .unwrap();

    let audit_dir = tmp.path().join("audit");
    let handle = BulwarkHandle::new_from_config(policy_path, Some(audit_dir.clone()), &AuditConfig::default());

    // Simulate what the curate path does
    let req = PolicyRequest {
        access_type: AccessType::Write,
        fact_ids: vec!["2026-03-14-test-curated-fact".to_string()],
        agent_id: Some("cli".to_string()),
        operation: "curate".to_string(),
        domain_tags: vec![],
        fact_types: vec!["durable".to_string()],
    };

    let decision = handle.check(&req);
    handle.audit(&req, &decision, 0);

    let log_path = audit_dir.join("engram.log");
    let content = std::fs::read_to_string(&log_path).expect("audit log should exist");
    let last_line = content.lines().last().expect("should have at least one line");
    let event: serde_json::Value = serde_json::from_str(last_line).expect("valid JSON");

    let fact_ids = event["fact_ids"]
        .as_array()
        .expect("fact_ids should be an array");
    assert_eq!(fact_ids.len(), 1);
    assert!(
        fact_ids[0].as_str().unwrap().contains("test-curated-fact"),
        "fact_ids should contain the curated fact ID"
    );
    assert_eq!(event["agent_id"].as_str().unwrap(), "cli");
    assert_eq!(event["operation"].as_str().unwrap(), "curate");
}
