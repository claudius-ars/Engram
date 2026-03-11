use std::path::Path;

use crate::fact_type::FactType;
use crate::frontmatter::RawFrontmatter;
use crate::validation::validate;

/// 1. Legacy ByteRover file (no Engram fields) parses without errors.
///    Only the expected "fact_type not set" warning should appear.
#[test]
fn test_legacy_byterover_file() {
    let yaml = r#"
title: "Kubernetes basics"
tags:
  - infra
  - k8s
keywords:
  - container
importance: 0.8
recency: 0.5
maturity: 0.9
accessCount: 5
updateCount: 2
createdAt: "2024-01-15T10:30:00Z"
updatedAt: "2024-06-01T12:00:00Z"
"#;
    let raw: RawFrontmatter = serde_yaml::from_str(yaml).unwrap();
    let path = Path::new(".brv/context-tree/infra/k8s.md");
    let record = validate(raw, path).unwrap();

    assert_eq!(record.title.as_deref(), Some("Kubernetes basics"));
    assert_eq!(record.tags, vec!["infra", "k8s"]);
    assert_eq!(record.importance, 0.8);
    assert_eq!(record.access_count, 5);
    assert_eq!(record.fact_type, FactType::Durable);
    assert_eq!(record.id, "infra/k8s");

    assert_eq!(record.warnings.len(), 1);
    assert!(record.warnings[0].message.contains("fact_type not set"));
}

/// 2. Full Engram frontmatter parses correctly — all fields present and valid.
#[test]
fn test_full_engram_frontmatter() {
    let yaml = r#"
title: "Deployment pipeline"
tags:
  - ci
keywords:
  - deploy
related:
  - infra/k8s
importance: 0.9
recency: 0.7
maturity: 0.8
accessCount: 10
updateCount: 3
createdAt: "2024-01-15T10:30:00Z"
updatedAt: "2024-06-01T12:00:00Z"
id: "ci/pipeline"
factType: state
validUntil: "2025-12-31T23:59:59Z"
causedBy:
  - infra/k8s
causes:
  - ci/deploy-v2
eventSequence: null
confidence: 0.95
domainTags:
  - infra
  - ci-cd
"#;
    let raw: RawFrontmatter = serde_yaml::from_str(yaml).unwrap();
    let path = Path::new("notes/pipeline.md");
    let record = validate(raw, path).unwrap();

    assert_eq!(record.id, "ci/pipeline");
    assert_eq!(record.fact_type, FactType::State);
    assert_eq!(record.confidence, 0.95);
    assert_eq!(record.caused_by, vec!["infra/k8s"]);
    assert_eq!(record.causes, vec!["ci/deploy-v2"]);
    assert_eq!(record.domain_tags, vec!["infra", "ci-cd"]);
    assert!(record.valid_until.is_some());
    assert!(record.warnings.is_empty());
}

/// 3. confidence out of range (1.1) produces ValidationError.
#[test]
fn test_confidence_out_of_range() {
    let yaml = r#"
confidence: 1.1
factType: durable
"#;
    let raw: RawFrontmatter = serde_yaml::from_str(yaml).unwrap();
    let path = Path::new("test.md");
    let result = validate(raw, path);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("confidence '1.1' out of range"));
}

/// 4. confidence = 0.05 produces a warning but not an error.
#[test]
fn test_confidence_very_low_warning() {
    let yaml = r#"
confidence: 0.05
factType: durable
"#;
    let raw: RawFrontmatter = serde_yaml::from_str(yaml).unwrap();
    let path = Path::new("test.md");
    let record = validate(raw, path).unwrap();

    assert_eq!(record.confidence, 0.05);
    assert!(record
        .warnings
        .iter()
        .any(|w| w.message.contains("confidence 0.05 is very low")));
}

/// 5. valid_until with no timezone produces a warning and assumes UTC.
#[test]
fn test_valid_until_no_timezone_warning() {
    let yaml = r#"
factType: state
validUntil: "2025-06-01T12:00:00"
"#;
    let raw: RawFrontmatter = serde_yaml::from_str(yaml).unwrap();
    let path = Path::new("test.md");
    let record = validate(raw, path).unwrap();

    assert!(record.valid_until.is_some());
    assert!(record
        .warnings
        .iter()
        .any(|w| w.message.contains("valid_until has no timezone specifier")));
}

/// 6. valid_until with invalid value produces ValidationError.
#[test]
fn test_valid_until_invalid() {
    let yaml = r#"
factType: state
validUntil: "not-a-date"
"#;
    let raw: RawFrontmatter = serde_yaml::from_str(yaml).unwrap();
    let path = Path::new("test.md");
    let result = validate(raw, path);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("valid_until 'not-a-date' is not a valid ISO 8601 datetime"));
}

/// 7. fact_type = durable + valid_until set produces a warning.
#[test]
fn test_durable_with_valid_until_warning() {
    let yaml = r#"
factType: durable
validUntil: "2025-12-31T23:59:59Z"
"#;
    let raw: RawFrontmatter = serde_yaml::from_str(yaml).unwrap();
    let path = Path::new("test.md");
    let record = validate(raw, path).unwrap();

    assert!(record
        .warnings
        .iter()
        .any(|w| w.message.contains("durable facts should not expire")));
}

/// 8. fact_type = event + no event_sequence produces a warning.
#[test]
fn test_event_without_sequence_warning() {
    let yaml = r#"
factType: event
"#;
    let raw: RawFrontmatter = serde_yaml::from_str(yaml).unwrap();
    let path = Path::new("test.md");
    let record = validate(raw, path).unwrap();

    assert!(record
        .warnings
        .iter()
        .any(|w| w.message.contains("event fact without event_sequence")));
}

/// 9. domain_tags with uppercase are silently normalized to lowercase.
#[test]
fn test_domain_tags_normalized_lowercase() {
    let yaml = r#"
factType: durable
domainTags:
  - Infra
  - CI-CD
"#;
    let raw: RawFrontmatter = serde_yaml::from_str(yaml).unwrap();
    let path = Path::new("test.md");
    let record = validate(raw, path).unwrap();

    assert_eq!(record.domain_tags, vec!["infra", "ci-cd"]);
}

/// 10. domain_tags with invalid characters produce ValidationError.
#[test]
fn test_domain_tags_invalid_characters() {
    let yaml = r#"
factType: durable
domainTags:
  - "valid-tag"
  - "invalid tag!"
"#;
    let raw: RawFrontmatter = serde_yaml::from_str(yaml).unwrap();
    let path = Path::new("test.md");
    let result = validate(raw, path);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("domain_tags contains invalid tag"));
}

/// 11. id derived correctly from a path under .brv/context-tree/.
#[test]
fn test_id_derived_from_context_tree_path() {
    let yaml = r#"
factType: durable
"#;
    let raw: RawFrontmatter = serde_yaml::from_str(yaml).unwrap();
    let path = Path::new(".brv/context-tree/infra/k8s.md");
    let record = validate(raw, path).unwrap();

    assert_eq!(record.id, "infra/k8s");
}

/// 12. caused_by self-reference produces ValidationError.
#[test]
fn test_caused_by_self_reference() {
    let yaml = r#"
id: "infra/k8s"
factType: durable
causedBy:
  - "infra/k8s"
"#;
    let raw: RawFrontmatter = serde_yaml::from_str(yaml).unwrap();
    let path = Path::new("test.md");
    let result = validate(raw, path);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("caused_by references its own id 'infra/k8s'"));
}

/// 13. Empty tags/keywords/domain_tags entries are filtered out silently.
#[test]
fn test_empty_entries_filtered() {
    let yaml = r#"
factType: durable
tags:
  - ""
  - "valid"
  - "  "
keywords:
  - ""
  - "real"
domainTags:
  - ""
  - "infra"
"#;
    let raw: RawFrontmatter = serde_yaml::from_str(yaml).unwrap();
    let path = Path::new("test.md");
    let record = validate(raw, path).unwrap();

    assert_eq!(record.tags, vec!["valid"]);
    assert_eq!(record.keywords, vec!["real"]);
    assert_eq!(record.domain_tags, vec!["infra"]);
}
