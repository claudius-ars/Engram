use engram_core::AuditConfig;
use engram_bulwark::{
    AccessType, BulwarkHandle, PolicyRequest, verify_audit_chain,
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

fn allow_all_policy() -> String {
    r#"
[[rules]]
name = "allow-all"
effect = "allow"
"#
    .to_string()
}

// ============================================================
// 1. Rotation at threshold
// ============================================================

#[test]
fn rotation_at_threshold() {
    let tmp = tempfile::tempdir().unwrap();
    let policy_path = tmp.path().join("bulwark.toml");
    let audit_dir = tmp.path().join("audit");

    std::fs::write(&policy_path, allow_all_policy()).unwrap();

    // max_log_bytes = 1 → rotate after first entry
    let handle = BulwarkHandle::new_from_config(policy_path, Some(audit_dir.clone()), &AuditConfig { max_log_bytes: 1, siem_endpoint: None, siem_token_env: None, siem_required: false });

    // Write first entry
    let req1 = make_request(AccessType::Read, "agent-0", "query");
    let decision1 = handle.check(&req1);
    handle.audit(&req1, &decision1, 0);

    // Write second entry — triggers rotation before writing
    let req2 = make_request(AccessType::Read, "agent-1", "query");
    let decision2 = handle.check(&req2);
    handle.audit(&req2, &decision2, 0);

    let log_path = audit_dir.join("engram.log");
    assert!(log_path.exists(), "current log should exist");

    // Find the rotated archive
    let archives: Vec<_> = std::fs::read_dir(&audit_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.starts_with("engram.log.") && name.len() > "engram.log.".len()
        })
        .collect();

    assert_eq!(
        archives.len(),
        1,
        "should have exactly one archive, got {:?}",
        archives.iter().map(|a| a.file_name()).collect::<Vec<_>>()
    );

    // Archive contains exactly 1 entry and verifies
    let archive_path = archives[0].path();
    let archive_count = verify_audit_chain(&archive_path).expect("archive should verify");
    assert_eq!(archive_count, 1, "archive should contain first entry");

    // Current log contains exactly 1 entry with fresh chain (prev_hash = 64 zeros)
    let current_count = verify_audit_chain(&log_path).expect("current log should verify");
    assert_eq!(current_count, 1, "current log should contain second entry");

    let content = std::fs::read_to_string(&log_path).unwrap();
    let first_line = content.lines().next().unwrap();
    let event: serde_json::Value = serde_json::from_str(first_line).unwrap();
    assert_eq!(
        event["prev_hash"].as_str().unwrap(),
        &"0".repeat(64),
        "fresh chain should start with 64 hex zeros"
    );
}

// ============================================================
// 2. No rotation when disabled
// ============================================================

#[test]
fn no_rotation_when_disabled() {
    let tmp = tempfile::tempdir().unwrap();
    let policy_path = tmp.path().join("bulwark.toml");
    let audit_dir = tmp.path().join("audit");

    std::fs::write(&policy_path, allow_all_policy()).unwrap();

    // max_log_bytes = 0 → no rotation
    let handle = BulwarkHandle::new_from_config(policy_path, Some(audit_dir.clone()), &AuditConfig { max_log_bytes: 0, siem_endpoint: None, siem_token_env: None, siem_required: false });

    for i in 0..10 {
        let req = make_request(AccessType::Read, &format!("agent-{}", i), "query");
        let decision = handle.check(&req);
        handle.audit(&req, &decision, 0);
    }

    // No archive files
    let archives: Vec<_> = std::fs::read_dir(&audit_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.starts_with("engram.log.") && name.len() > "engram.log.".len()
        })
        .collect();

    assert!(archives.is_empty(), "no archives should exist when rotation disabled");

    let log_path = audit_dir.join("engram.log");
    let count = verify_audit_chain(&log_path).expect("log should verify");
    assert_eq!(count, 10);
}

// ============================================================
// 3. Rotation archive sealed — multiple rotations
// ============================================================

#[test]
fn rotation_archive_sealed() {
    let tmp = tempfile::tempdir().unwrap();
    let policy_path = tmp.path().join("bulwark.toml");
    let audit_dir = tmp.path().join("audit");

    std::fs::write(&policy_path, allow_all_policy()).unwrap();

    // max_log_bytes = 1 → rotate before every write after the first
    let handle = BulwarkHandle::new_from_config(policy_path, Some(audit_dir.clone()), &AuditConfig { max_log_bytes: 1, siem_endpoint: None, siem_token_env: None, siem_required: false });

    // Write three entries: triggers two rotations
    for i in 0..3 {
        let req = make_request(AccessType::Read, &format!("agent-{}", i), "query");
        let decision = handle.check(&req);
        handle.audit(&req, &decision, 0);
        // Small sleep to ensure distinct archive timestamps
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    let log_path = audit_dir.join("engram.log");

    let mut archives: Vec<_> = std::fs::read_dir(&audit_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.starts_with("engram.log.") && name.len() > "engram.log.".len()
        })
        .collect();

    assert_eq!(
        archives.len(),
        2,
        "should have two archives, got {:?}",
        archives.iter().map(|a| a.file_name()).collect::<Vec<_>>()
    );

    // Each archive passes verify_audit_chain independently
    archives.sort_by_key(|a| a.file_name());
    for archive in &archives {
        let count = verify_audit_chain(&archive.path())
            .unwrap_or_else(|_| panic!("archive {:?} should verify", archive.file_name()));
        assert_eq!(count, 1, "each archive should contain exactly 1 entry");
    }

    // Current log also verifies
    let current_count = verify_audit_chain(&log_path).expect("current log should verify");
    assert_eq!(current_count, 1, "current log should contain the last entry");
}
