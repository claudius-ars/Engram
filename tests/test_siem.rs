use engram_bulwark::{AccessType, AuditWriter, PolicyDecision, PolicyRequest};

fn make_request() -> PolicyRequest {
    PolicyRequest {
        access_type: AccessType::Read,
        fact_ids: vec![],
        agent_id: Some("test-agent".to_string()),
        operation: "query".to_string(),
        domain_tags: vec![],
        fact_types: vec![],
    }
}

// ============================================================
// 1. SIEM emission called — mock receives POST with auth header
// ============================================================

#[test]
fn siem_emission_called() {
    let mut server = mockito::Server::new();
    let mock = server
        .mock("POST", "/ingest")
        .match_header("Authorization", "Bearer test-token-value")
        .match_header("Content-Type", "application/json")
        .with_status(200)
        .create();

    let tmp = tempfile::tempdir().unwrap();
    let log_path = tmp.path().join("engram.log");

    // Set the env var for token resolution
    let env_var = format!("ENGRAM_TEST_SIEM_TOKEN_{}", std::process::id());
    std::env::set_var(&env_var, "test-token-value");

    let mut writer = AuditWriter::new(
        log_path.clone(),
        0,
        Some(format!("{}/ingest", server.url())),
        Some(&env_var),
        false,
    );

    let req = make_request();
    let decision = PolicyDecision::Allow;
    writer.append(&req, &decision, 42).unwrap();

    std::env::remove_var(&env_var);

    mock.assert();

    // Verify disk write also succeeded
    let content = std::fs::read_to_string(&log_path).unwrap();
    assert!(!content.is_empty());
    let event: serde_json::Value = serde_json::from_str(content.lines().next().unwrap()).unwrap();
    assert_eq!(event["agent_id"], "test-agent");
}

// ============================================================
// 2. SIEM 5xx retried once
// ============================================================

#[test]
fn siem_5xx_retried() {
    let mut server = mockito::Server::new();

    // First call: 500
    let mock_500 = server
        .mock("POST", "/ingest")
        .with_status(500)
        .create();

    // Second call (retry): 200
    let mock_200 = server
        .mock("POST", "/ingest")
        .with_status(200)
        .create();

    let tmp = tempfile::tempdir().unwrap();
    let log_path = tmp.path().join("engram.log");

    let env_var = format!("ENGRAM_TEST_SIEM_5XX_{}", std::process::id());
    std::env::set_var(&env_var, "token-5xx");

    let mut writer = AuditWriter::new(
        log_path,
        0,
        Some(format!("{}/ingest", server.url())),
        Some(&env_var),
        false,
    );

    let req = make_request();
    let decision = PolicyDecision::Allow;
    writer.append(&req, &decision, 0).unwrap();

    std::env::remove_var(&env_var);

    // Both mocks should have been hit (initial + retry)
    mock_500.assert();
    mock_200.assert();
}

// ============================================================
// 3. SIEM 4xx not retried
// ============================================================

#[test]
fn siem_4xx_not_retried() {
    let mut server = mockito::Server::new();
    let mock = server
        .mock("POST", "/ingest")
        .with_status(400)
        .expect(1) // exactly once, no retry
        .create();

    let tmp = tempfile::tempdir().unwrap();
    let log_path = tmp.path().join("engram.log");

    let env_var = format!("ENGRAM_TEST_SIEM_4XX_{}", std::process::id());
    std::env::set_var(&env_var, "token-4xx");

    let mut writer = AuditWriter::new(
        log_path,
        0,
        Some(format!("{}/ingest", server.url())),
        Some(&env_var),
        false,
    );

    let req = make_request();
    let decision = PolicyDecision::Allow;
    writer.append(&req, &decision, 0).unwrap();

    std::env::remove_var(&env_var);

    mock.assert();
}

// ============================================================
// 4. SIEM failure does not fail audit append
// ============================================================

#[test]
fn siem_failure_does_not_fail_audit() {
    // Use a port that nothing is listening on
    let tmp = tempfile::tempdir().unwrap();
    let log_path = tmp.path().join("engram.log");

    let env_var = format!("ENGRAM_TEST_SIEM_FAIL_{}", std::process::id());
    std::env::set_var(&env_var, "token-fail");

    let mut writer = AuditWriter::new(
        log_path.clone(),
        0,
        Some("http://127.0.0.1:1".to_string()), // unreachable
        Some(&env_var),
        false,
    );

    let req = make_request();
    let decision = PolicyDecision::Allow;

    // append() should still return Ok despite SIEM failure
    let result = writer.append(&req, &decision, 0);
    assert!(result.is_ok(), "audit append should succeed even when SIEM fails");

    std::env::remove_var(&env_var);

    // Disk file should contain the entry
    let content = std::fs::read_to_string(&log_path).unwrap();
    assert!(!content.is_empty(), "disk log should contain the entry");
}

// ============================================================
// 5. SIEM disabled when token env var missing
// ============================================================

#[test]
fn siem_disabled_when_token_missing() {
    let mut server = mockito::Server::new();
    let mock = server
        .mock("POST", "/ingest")
        .expect(0) // should NOT be called
        .create();

    let tmp = tempfile::tempdir().unwrap();
    let log_path = tmp.path().join("engram.log");

    // Use a var name that definitely doesn't exist
    let env_var = format!("ENGRAM_NONEXISTENT_VAR_{}", std::process::id());
    std::env::remove_var(&env_var); // ensure it's unset

    let mut writer = AuditWriter::new(
        log_path.clone(),
        0,
        Some(format!("{}/ingest", server.url())),
        Some(&env_var),
        false,
    );

    let req = make_request();
    let decision = PolicyDecision::Allow;
    writer.append(&req, &decision, 0).unwrap();

    mock.assert(); // expect(0) → no requests made

    // Disk write still works
    let content = std::fs::read_to_string(&log_path).unwrap();
    assert!(!content.is_empty());
}

// ============================================================
// 6. SIEM reachability — no SIEM configured → Ok(())
// ============================================================

#[test]
fn test_siem_reachability_no_siem_configured() {
    let tmp = tempfile::tempdir().unwrap();
    let log_path = tmp.path().join("engram.log");

    let writer = AuditWriter::new(
        log_path,
        0,
        None,   // no SIEM endpoint
        None,
        false,
    );

    assert!(writer.verify_siem_reachability().is_ok());
}

// ============================================================
// 7. SIEM reachability — not required + unreachable → Ok(())
// ============================================================

#[test]
fn test_siem_reachability_siem_not_required_unreachable() {
    let mut server = mockito::Server::new();
    let _mock = server
        .mock("HEAD", "/ingest")
        .with_status(503)
        .create();

    let tmp = tempfile::tempdir().unwrap();
    let log_path = tmp.path().join("engram.log");

    let env_var = format!("ENGRAM_TEST_REACH_NR_{}", std::process::id());
    std::env::set_var(&env_var, "token-nr");

    let writer = AuditWriter::new(
        log_path,
        0,
        Some(format!("{}/ingest", server.url())),
        Some(&env_var),
        false, // siem_required = false
    );

    // Should return Ok even though endpoint returned 503
    assert!(writer.verify_siem_reachability().is_ok());

    std::env::remove_var(&env_var);
}

// ============================================================
// 8. SIEM reachability — required + unreachable → Err
// ============================================================

#[test]
fn test_siem_reachability_siem_required_unreachable() {
    let mut server = mockito::Server::new();
    let _mock = server
        .mock("HEAD", "/ingest")
        .with_status(503)
        .create();

    let tmp = tempfile::tempdir().unwrap();
    let log_path = tmp.path().join("engram.log");

    let env_var = format!("ENGRAM_TEST_REACH_RU_{}", std::process::id());
    std::env::set_var(&env_var, "token-ru");

    let writer = AuditWriter::new(
        log_path,
        0,
        Some(format!("{}/ingest", server.url())),
        Some(&env_var),
        true, // siem_required = true
    );

    let result = writer.verify_siem_reachability();
    assert!(result.is_err());
    let msg = result.unwrap_err();
    assert!(msg.contains("SIEM endpoint unreachable"), "error should mention unreachable: {}", msg);

    std::env::remove_var(&env_var);
}

// ============================================================
// 9. SIEM reachability — required + reachable → Ok(())
// ============================================================

#[test]
fn test_siem_reachability_siem_required_reachable() {
    let mut server = mockito::Server::new();
    let _mock = server
        .mock("HEAD", "/ingest")
        .with_status(200)
        .create();

    let tmp = tempfile::tempdir().unwrap();
    let log_path = tmp.path().join("engram.log");

    let env_var = format!("ENGRAM_TEST_REACH_RR_{}", std::process::id());
    std::env::set_var(&env_var, "token-rr");

    let writer = AuditWriter::new(
        log_path,
        0,
        Some(format!("{}/ingest", server.url())),
        Some(&env_var),
        true, // siem_required = true
    );

    assert!(writer.verify_siem_reachability().is_ok());

    std::env::remove_var(&env_var);
}
