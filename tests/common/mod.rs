use std::fs;
use std::path::Path;

use engram_bulwark::BulwarkHandle;
use engram_compiler::{compile_context_tree, CompileResult};
use tempfile::TempDir;

/// Create a temp workspace with an initialized .brv/context-tree/
pub fn temp_workspace() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join(".brv/context-tree")).unwrap();
    dir
}

/// Write a .md file into the workspace context-tree.
pub fn write_fact(root: &Path, filename: &str, content: &str) {
    let path = root.join(".brv/context-tree").join(filename);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

/// Compile the workspace and assert no index error.
pub fn compile_clean(root: &Path) -> CompileResult {
    let bulwark = BulwarkHandle::new_stub();
    let result = compile_context_tree(root, true, &bulwark);
    assert!(
        result.index_error.is_none(),
        "compile_clean: unexpected index error: {:?}",
        result.index_error
    );
    result
}

/// Write the state file's dirty flag to true.
pub fn set_dirty(root: &Path) {
    let state_path = root.join(".brv/index/state");
    let raw = fs::read_to_string(&state_path).unwrap();
    let mut val: serde_json::Value = serde_json::from_str(&raw).unwrap();
    val["dirty"] = serde_json::Value::Bool(true);
    val["dirty_since"] =
        serde_json::Value::String(chrono::Utc::now().to_rfc3339());
    fs::write(
        &state_path,
        serde_json::to_string_pretty(&val).unwrap(),
    )
    .unwrap();
}

/// A minimal valid durable fact.
pub fn durable_fact(title: &str, body: &str) -> String {
    format!(
        r#"---
title: "{title}"
factType: durable
confidence: 1.0
importance: 0.8
recency: 0.9
tags: [test]
---

{body}
"#
    )
}

/// A minimal valid state fact.
pub fn state_fact(title: &str, body: &str) -> String {
    format!(
        r#"---
title: "{title}"
factType: state
confidence: 0.9
importance: 0.7
recency: 0.8
tags: [test, state]
---

{body}
"#
    )
}

/// A fact WITHOUT explicit factType (will default to durable without --classify).
pub fn unclassified_fact(title: &str, body: &str) -> String {
    format!(
        r#"---
title: "{title}"
confidence: 1.0
importance: 0.8
recency: 0.9
tags: [test]
---

{body}
"#
    )
}

/// Compile with --classify flag.
pub fn compile_with_classify(root: &Path) -> CompileResult {
    let bulwark = BulwarkHandle::new_stub();
    let config = engram_core::CompileConfig {
        classify: true,
        ..engram_core::CompileConfig::default()
    };
    let result = engram_compiler::compile_context_tree_with_config(root, true, &bulwark, &config);
    assert!(
        result.index_error.is_none(),
        "compile_with_classify: unexpected index error: {:?}",
        result.index_error
    );
    result
}

/// Compile with --incremental flag.
pub fn compile_incremental(root: &Path) -> engram_compiler::CompileResult {
    let bulwark = BulwarkHandle::new_stub();
    let config = engram_core::CompileConfig::default();
    engram_compiler::compile_incremental(root, &bulwark, &config)
}

/// A minimal valid event fact.
pub fn event_fact(title: &str, body: &str, seq: i64) -> String {
    format!(
        r#"---
title: "{title}"
factType: event
eventSequence: {seq}
confidence: 0.8
importance: 0.6
recency: 0.7
tags: [test, event]
---

{body}
"#
    )
}
