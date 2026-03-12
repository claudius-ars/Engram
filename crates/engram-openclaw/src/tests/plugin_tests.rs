use std::path::Path;

use chrono::Utc;
use engram_bulwark::BulwarkHandle;

use crate::{enrich_once, EngramPlugin, EnrichOptions};

/// Helper: compile a temp dir with specified fixture files and return TempDir.
fn compile_fixtures(fixtures: &[&str]) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let context_tree = tmp.path().join(".brv").join("context-tree");
    std::fs::create_dir_all(&context_tree).unwrap();

    let fixtures_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("engram-compiler")
        .join("tests")
        .join("fixtures");

    for name in fixtures {
        std::fs::copy(fixtures_dir.join(name), context_tree.join(name)).unwrap();
    }

    engram_compiler::compile_context_tree(tmp.path(), true, &BulwarkHandle::new_stub());

    // Write permissive config so scoring thresholds don't filter fixture results
    std::fs::write(
        tmp.path().join(".brv/engram.toml"),
        "[query]\nscore_threshold = 0.0\nscore_gap = 0.0\n",
    ).unwrap();

    tmp
}

/// Helper: set dirty flag in state file.
fn set_dirty(root: &Path) {
    let state_path = root.join(".brv").join("index").join("state");
    let content = std::fs::read_to_string(&state_path).unwrap();
    let mut state: serde_json::Value = serde_json::from_str(&content).unwrap();
    state["dirty"] = serde_json::Value::Bool(true);
    state["dirty_since"] = serde_json::Value::String(Utc::now().to_rfc3339());
    std::fs::write(&state_path, serde_json::to_string_pretty(&state).unwrap()).unwrap();
}

// --- Test 9: plugin enrich no index ---
#[test]
fn test_plugin_enrich_no_index() {
    let tmp = tempfile::tempdir().unwrap();
    let mut plugin = EngramPlugin::new(tmp.path().to_path_buf(), EnrichOptions::default());
    let result = plugin.enrich("anything");
    assert!(!result.from_index);
    assert_eq!(result.fact_count, 0);
    assert!(result.cache_tier.is_none());
}

// --- Test 10: plugin enrich with index ---
#[test]
fn test_plugin_enrich_with_index() {
    let tmp = compile_fixtures(&["valid_legacy.md", "valid_engram.md"]);
    let mut plugin = EngramPlugin::new(tmp.path().to_path_buf(), EnrichOptions::default());
    let result = plugin.enrich("Legacy");
    assert!(result.from_index);
    assert!(result.fact_count >= 1);
    assert!(result.context_block.contains("## Engram Context (Auto-Enriched)"));
}

// --- Test 11: plugin cache reuse ---
#[test]
fn test_plugin_cache_reuse() {
    let tmp = compile_fixtures(&["valid_legacy.md", "valid_engram.md"]);
    let mut plugin = EngramPlugin::new(tmp.path().to_path_buf(), EnrichOptions::default());

    // First call populates cache
    let r1 = plugin.enrich("Legacy");
    assert!(r1.from_index);
    assert_eq!(r1.cache_tier, Some(2));

    // Second call should hit Tier 0 cache
    let r2 = plugin.enrich("Legacy");
    assert!(r2.from_index);
    assert_eq!(r2.cache_tier, Some(0));
}

// --- Test 12: plugin fallback message ---
#[test]
fn test_plugin_fallback_message() {
    let tmp = tempfile::tempdir().unwrap();
    let options = EnrichOptions {
        fallback_message: Some("No facts yet.".to_string()),
        ..EnrichOptions::default()
    };
    let mut plugin = EngramPlugin::new(tmp.path().to_path_buf(), options);
    let result = plugin.enrich("anything");
    assert!(!result.from_index);
    assert_eq!(result.context_block, "No facts yet.");
}

// --- Test 13: enrich_once convenience ---
#[test]
fn test_enrich_once_convenience() {
    let tmp = tempfile::tempdir().unwrap();
    let result = enrich_once(tmp.path(), "anything", EnrichOptions::default());
    assert!(!result.from_index);
    assert_eq!(result.fact_count, 0);
}

// --- Test 14: stale warning ---
#[test]
fn test_format_stale_warning() {
    let tmp = compile_fixtures(&["valid_legacy.md"]);
    let mut plugin = EngramPlugin::new(tmp.path().to_path_buf(), EnrichOptions::default());

    set_dirty(tmp.path());

    let result = plugin.enrich("Legacy");
    assert!(result.stale);
}
