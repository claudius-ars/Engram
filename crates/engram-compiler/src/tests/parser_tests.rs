use std::path::{Path, PathBuf};

use engram_core::FactType;

use engram_bulwark::BulwarkHandle;

use crate::parser::{extract_frontmatter, parse_all, parse_file, ParseError};
use crate::compile_context_tree;

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

// --- Test 1: extract_frontmatter with frontmatter ---
#[test]
fn test_extract_frontmatter_with_frontmatter() {
    let content = "---\ntitle: \"My Fact\"\nfactType: state\n---\n\n## Body\nSome content here.\n";
    let (yaml, body) = extract_frontmatter(content);
    assert!(yaml.is_some());
    let yaml = yaml.unwrap();
    assert!(yaml.contains("title: \"My Fact\""));
    assert!(yaml.contains("factType: state"));
    assert!(body.contains("## Body"));
    assert!(body.contains("Some content here."));
}

// --- Test 2: extract_frontmatter without frontmatter ---
#[test]
fn test_extract_frontmatter_no_frontmatter() {
    let content = "## Just a body\nNo frontmatter here.\n";
    let (yaml, body) = extract_frontmatter(content);
    assert!(yaml.is_none());
    assert_eq!(body, content);
}

// --- Test 3: extract_frontmatter empty file ---
#[test]
fn test_extract_frontmatter_empty() {
    let (yaml, body) = extract_frontmatter("");
    assert!(yaml.is_none());
    assert_eq!(body, "");
}

// --- Test 4: parse_file valid_legacy.md ---
#[test]
fn test_parse_file_valid_legacy() {
    let path = fixture_path("valid_legacy.md");
    let record = parse_file(&path).unwrap();

    assert_eq!(record.title.as_deref(), Some("Legacy Fact"));
    assert!(record.tags.contains(&"rust".to_string()));
    assert!(record.tags.contains(&"systems".to_string()));
    assert_eq!(record.importance, 0.8);
    assert_eq!(record.access_count, 5);
    assert_eq!(record.fact_type, FactType::Durable);

    // Exactly one warning: fact_type not set
    assert_eq!(record.warnings.len(), 1);
    assert!(record.warnings[0].message.contains("fact_type not set"));
}

// --- Test 5: parse_file valid_engram.md ---
#[test]
fn test_parse_file_valid_engram() {
    let path = fixture_path("valid_engram.md");
    let record = parse_file(&path).unwrap();

    assert_eq!(record.fact_type, FactType::State);
    assert_eq!(record.confidence, 0.9);
    assert!(record.domain_tags.contains(&"infra:k8s".to_string()));
    assert!(record.domain_tags.contains(&"platform".to_string()));
    assert!(record.valid_until.is_some());
    assert!(record.warnings.is_empty());
}

// --- Test 6: parse_file no_frontmatter.md ---
#[test]
fn test_parse_file_no_frontmatter() {
    let path = fixture_path("no_frontmatter.md");
    let record = parse_file(&path).unwrap();

    assert_eq!(record.importance, 1.0);
    assert_eq!(record.confidence, 1.0);
    assert!(record
        .warnings
        .iter()
        .any(|w| w.message.contains("no frontmatter found")));
    assert!(record
        .warnings
        .iter()
        .any(|w| w.message.contains("fact_type not set")));
}

// --- Test 7: parse_file invalid_yaml.md ---
#[test]
fn test_parse_file_invalid_yaml() {
    let path = fixture_path("invalid_yaml.md");
    let result = parse_file(&path);
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        ParseError::FrontmatterError { .. }
    ));
}

// --- Test 8: parse_file invalid_field.md ---
#[test]
fn test_parse_file_invalid_field() {
    let path = fixture_path("invalid_field.md");
    let result = parse_file(&path);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("confidence"));
}

// --- Test 9: parse_all with mixed files ---
#[test]
fn test_parse_all_mixed() {
    let paths = vec![
        fixture_path("valid_legacy.md"),
        fixture_path("valid_engram.md"),
        fixture_path("no_frontmatter.md"),
        fixture_path("invalid_yaml.md"),
        fixture_path("invalid_field.md"),
    ];

    let result = parse_all(paths);

    assert_eq!(result.file_count, 5);
    assert_eq!(result.records.len(), 3);
    assert_eq!(result.errors.len(), 2);
    assert_eq!(result.error_count, 2);
}

// --- Test 10: compile_context_tree missing context tree ---
#[test]
fn test_compile_missing_context_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let result = compile_context_tree(tmp.path(), false, &BulwarkHandle::new_stub());

    assert_eq!(result.parse_result.records.len(), 0);
    assert_eq!(result.parse_result.errors.len(), 1);
    assert_eq!(result.parse_result.error_count, 1);
}

// --- Test 11: compile_context_tree with fixtures copied ---
#[test]
fn test_compile_with_fixtures() {
    let tmp = tempfile::tempdir().unwrap();
    let context_tree = tmp.path().join(".brv").join("context-tree");
    std::fs::create_dir_all(&context_tree).unwrap();

    // Copy fixture files into the context tree
    let fixtures_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures");

    for name in &[
        "valid_legacy.md",
        "valid_engram.md",
        "no_frontmatter.md",
        "invalid_yaml.md",
        "invalid_field.md",
    ] {
        std::fs::copy(fixtures_dir.join(name), context_tree.join(name)).unwrap();
    }

    let result = compile_context_tree(tmp.path(), false, &BulwarkHandle::new_stub());

    assert_eq!(result.parse_result.file_count, 5);
    assert_eq!(result.parse_result.records.len(), 3);
    assert_eq!(result.parse_result.errors.len(), 2);
    assert_eq!(result.parse_result.error_count, 2);
}

// --- Test 12: bench_parse_500_files ---
#[test]
fn bench_parse_500_files() {
    let tmp = tempfile::tempdir().unwrap();
    let context_tree = tmp.path().join(".brv").join("context-tree");
    std::fs::create_dir_all(&context_tree).unwrap();

    for i in 0..500 {
        let has_fact_type = i % 2 == 0;
        let has_confidence = i % 4 == 0;
        let has_domain_tags = i % 4 == 1;

        let tag_count = (i % 3) + 1;
        let tags: Vec<String> = (0..tag_count).map(|t| format!("tag{}", t)).collect();
        let tags_yaml: String = tags.iter().map(|t| format!("\n  - {}", t)).collect();

        let importance = 0.5 + (i % 50) as f64 * 0.01;

        let mut yaml = format!(
            "---\ntitle: \"Fact {i}\"\ntags:{tags_yaml}\nimportance: {importance:.2}\n"
        );

        if has_fact_type {
            let ft = match i % 6 {
                0 => "durable",
                2 => "state",
                4 => "event",
                _ => "durable",
            };
            yaml.push_str(&format!("factType: {ft}\n"));
            if ft == "event" {
                yaml.push_str(&format!("eventSequence: {i}\n"));
            }
        }

        if has_confidence {
            let conf = 0.5 + (i % 50) as f64 * 0.01;
            yaml.push_str(&format!("confidence: {conf:.2}\n"));
        }

        if has_domain_tags {
            yaml.push_str("domainTags:\n  - domain:alpha\n  - domain:beta\n");
        }

        yaml.push_str("---\n\n## Raw Concept\nThis is the body of fact ");
        yaml.push_str(&i.to_string());
        yaml.push_str(". It contains some text for full-text indexing.\n");

        let filename = format!("fact{:04}.md", i);
        std::fs::write(context_tree.join(&filename), &yaml).unwrap();
    }

    let start = std::time::Instant::now();
    let result = compile_context_tree(tmp.path(), false, &BulwarkHandle::new_stub());
    let elapsed = start.elapsed();

    assert_eq!(result.parse_result.file_count, 500);
    assert_eq!(result.parse_result.errors.len(), 0, "unexpected errors: {:?}", result.parse_result.errors);
    assert_eq!(result.parse_result.records.len(), 500);

    eprintln!("bench_parse_500_files: {}ms", elapsed.as_millis());

    if elapsed.as_millis() > 2000 {
        eprintln!(
            "PERF WARNING: bench_parse_500_files took {}ms — \
            expected under 1000ms on developer hardware. \
            This may indicate a performance regression.",
            elapsed.as_millis()
        );
    }
}
