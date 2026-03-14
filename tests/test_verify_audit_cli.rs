use engram_bulwark::{AccessType, AuditWriter, PolicyDecision, PolicyRequest};

fn make_request() -> PolicyRequest {
    PolicyRequest {
        access_type: AccessType::Read,
        fact_id: None,
        agent_id: Some("cli-test-agent".to_string()),
        operation: "query".to_string(),
        domain_tags: vec![],
        fact_types: vec![],
    }
}

fn seed_entries(log_path: &std::path::Path, count: usize) {
    let mut writer = AuditWriter::new(
        log_path.to_path_buf(),
        0,
        None,
        None,
        false,
    );
    let req = make_request();
    let decision = PolicyDecision::Allow;
    for _ in 0..count {
        writer.append(&req, &decision, 0).unwrap();
    }
}

/// Locate the `engram` binary in the target directory.
/// When `cargo test --workspace` runs, workspace binaries are built
/// alongside test binaries under the same target profile directory.
fn engram_binary() -> std::path::PathBuf {
    let test_exe = std::env::current_exe().expect("cannot determine test binary path");
    // test binary is at target/debug/deps/test_xxx; engram is at target/debug/engram
    let target_dir = test_exe
        .parent().unwrap()  // deps/
        .parent().unwrap(); // debug/ (or release/)
    target_dir.join("engram")
}

fn run_verify_audit(log_path: &std::path::Path) -> std::process::Output {
    let binary = engram_binary();
    std::process::Command::new(&binary)
        .args(["query", "--verify-audit", "--log", log_path.to_str().unwrap()])
        .output()
        .unwrap_or_else(|e| panic!("failed to run {}: {}", binary.display(), e))
}

// ============================================================
// 1. Valid chain — exit 0, stdout contains entry count
// ============================================================

#[test]
fn test_verify_audit_valid_chain() {
    let tmp = tempfile::tempdir().unwrap();
    let log_path = tmp.path().join("engram.log");

    seed_entries(&log_path, 3);

    let output = run_verify_audit(&log_path);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert_eq!(output.status.code(), Some(0), "expected exit 0, stderr: {}", String::from_utf8_lossy(&output.stderr));
    assert!(stdout.contains("3"), "stdout should contain entry count: {}", stdout);
    assert!(stdout.contains("valid") || stdout.contains("Valid"), "stdout should mention valid: {}", stdout);
}

// ============================================================
// 2. Broken chain — exit 1, stderr contains mismatch info
// ============================================================

#[test]
fn test_verify_audit_broken_chain() {
    let tmp = tempfile::tempdir().unwrap();
    let log_path = tmp.path().join("engram.log");

    seed_entries(&log_path, 3);

    // Corrupt the prev_hash of the third entry (line 3)
    let content = std::fs::read_to_string(&log_path).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines.len(), 3, "expected 3 lines in log");

    // Replace the first hex char of prev_hash in line 3 with a different char
    let corrupted_line = if lines[2].contains("\"prev_hash\":\"0") {
        lines[2].replacen("\"prev_hash\":\"0", "\"prev_hash\":\"f", 1)
    } else {
        lines[2].replacen("\"prev_hash\":\"", "\"prev_hash\":\"00", 1)
    };

    let corrupted = format!("{}\n{}\n{}\n", lines[0], lines[1], corrupted_line);
    std::fs::write(&log_path, corrupted).unwrap();

    let output = run_verify_audit(&log_path);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(1), "expected exit 1 on broken chain");
    assert!(
        stderr.contains("HashMismatch") || stderr.contains("mismatch") || stderr.contains("failed"),
        "stderr should indicate hash mismatch: {}",
        stderr
    );
}

// ============================================================
// 3. Empty log — exit 0, stdout contains 0
// ============================================================

#[test]
fn test_verify_audit_empty_log() {
    let tmp = tempfile::tempdir().unwrap();
    let log_path = tmp.path().join("engram.log");

    std::fs::write(&log_path, b"").unwrap();

    let output = run_verify_audit(&log_path);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert_eq!(output.status.code(), Some(0), "expected exit 0 for empty log, stderr: {}", String::from_utf8_lossy(&output.stderr));
    assert!(stdout.contains("0"), "stdout should contain 0 entries: {}", stdout);
}
