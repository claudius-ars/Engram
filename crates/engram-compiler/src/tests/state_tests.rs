use crate::state::{fresh_state, read_state, write_state, COMPILER_VERSION, STATE_VERSION};

// --- Test 1: fresh_state defaults ---
#[test]
fn test_fresh_state_defaults() {
    let state = fresh_state(500, 312);

    assert_eq!(state.version, STATE_VERSION);
    assert!(!state.dirty);
    assert_eq!(state.generation, 1);
    assert!(state.dirty_since.is_none());
    assert!(state.last_compiled_at.is_some());
    assert_eq!(state.last_compiled_duration_ms, Some(312));
    assert_eq!(state.compiled_file_count, 500);
    assert_eq!(state.compiler_version, COMPILER_VERSION);
}

// --- Test 2: write/read roundtrip ---
#[test]
fn test_write_read_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let index_dir = tmp.path();

    let state = fresh_state(42, 100);
    write_state(index_dir, &state).unwrap();
    let read_back = read_state(index_dir).unwrap();

    assert_eq!(read_back.version, state.version);
    assert_eq!(read_back.dirty, state.dirty);
    assert_eq!(read_back.generation, state.generation);
    assert!(read_back.dirty_since.is_none());
    assert_eq!(
        read_back.last_compiled_duration_ms,
        state.last_compiled_duration_ms
    );
    assert_eq!(read_back.compiled_file_count, state.compiled_file_count);
    assert_eq!(read_back.compiler_version, state.compiler_version);
}

// --- Test 3: atomic write leaves no .tmp file ---
#[test]
fn test_write_is_atomic() {
    let tmp = tempfile::tempdir().unwrap();
    let index_dir = tmp.path();

    let state = fresh_state(10, 50);
    write_state(index_dir, &state).unwrap();

    let tmp_path = index_dir.join("state.tmp");
    assert!(!tmp_path.exists(), ".tmp file should not exist after write");
}

// --- Test 4: read missing returns error ---
#[test]
fn test_read_missing_returns_error() {
    let tmp = tempfile::tempdir().unwrap();
    let result = read_state(tmp.path());
    assert!(result.is_err());
}

// --- Test 5: generation increment ---
#[test]
fn test_generation_increment() {
    let tmp = tempfile::tempdir().unwrap();
    let index_dir = tmp.path();

    // First compile: generation 1
    let state1 = fresh_state(10, 50);
    assert_eq!(state1.generation, 1);
    write_state(index_dir, &state1).unwrap();

    // Second compile: read previous, increment
    let prev = read_state(index_dir).unwrap();
    let mut state2 = fresh_state(12, 60);
    state2.generation = prev.generation + 1;
    write_state(index_dir, &state2).unwrap();

    let read_back = read_state(index_dir).unwrap();
    assert_eq!(read_back.generation, 2);
}
