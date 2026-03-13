use engram_bulwark::{
    AccessType, BulwarkHandle, ChainError, PolicyDecision, PolicyRequest, verify_audit_chain,
};

fn make_request(access_type: AccessType, agent: &str, operation: &str) -> PolicyRequest {
    PolicyRequest {
        access_type,
        fact_id: None,
        agent_id: Some(agent.to_string()),
        operation: operation.to_string(),
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

    let handle = BulwarkHandle::new_from_config(policy_path, Some(audit_dir.clone()));

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

    let handle = BulwarkHandle::new_from_config(policy_path, Some(audit_dir.clone()));

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

    let handle = BulwarkHandle::new_from_config(policy_path, Some(audit_dir.clone()));

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

    let handle = BulwarkHandle::new_from_config(policy_path, Some(audit_dir.clone()));

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

    let handle = BulwarkHandle::new_from_config(policy_path, Some(audit_dir.clone()));

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
