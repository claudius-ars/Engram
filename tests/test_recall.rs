#[allow(unused)]
mod common;

use engram_bulwark::BulwarkHandle;
use engram_core::WorkspaceConfig;
use engram_query::{ExactCache, FuzzyCache, QueryOptions};

use common::{compile_clean, durable_fact, temp_workspace, write_fact};

fn default_query_options() -> QueryOptions {
    QueryOptions::default()
}

fn query_helper(
    root: &std::path::Path,
    query_str: &str,
    cache: &mut ExactCache,
    fuzzy_cache: &mut FuzzyCache,
) -> engram_query::QueryResult {
    let bulwark = BulwarkHandle::new_stub();
    let config = WorkspaceConfig {
        score_threshold: 0.0,
        score_gap: 0.0,
        ..WorkspaceConfig::default()
    };
    engram_query::query(root, query_str, default_query_options(), cache, fuzzy_cache, &bulwark, &config)
        .expect("query should succeed")
}

// --- Legacy format fixture content ---

const LEGACY_AUTOSCALER: &str = r#"---
title: "Kubernetes Node Autoscaler"
tags: [infrastructure, kubernetes, autoscaling]
keywords: [hpa, vpa, cluster-autoscaler]
importance: 0.8
recency: 0.7
maturity: 0.6
---

The Kubernetes cluster autoscaler adjusts node count based on
pending pods and resource utilization.
"#;

const LEGACY_NETWORKING: &str = r#"---
title: "Service Mesh Networking"
tags: [infrastructure, networking, istio]
keywords: [envoy, sidecar, mtls]
importance: 0.7
recency: 0.6
maturity: 0.5
---

Istio service mesh provides mTLS, traffic management, and
observability for microservices communication.
"#;

const LEGACY_LOW_IMPORTANCE: &str = r#"---
title: "Kubernetes Legacy Notes"
tags: [infrastructure, kubernetes]
importance: 0.4
recency: 0.6
---

Legacy notes about Kubernetes v1.18 cluster setup procedures.
"#;

const LEGACY_WITH_KEYWORDS: &str = r#"---
title: "CI/CD Deployment Rollout"
tags: [cicd, deployment]
keywords: [deployment, rollout, canary, bluegreen]
importance: 0.7
recency: 0.8
---

The deployment pipeline supports canary and blue-green rollout
strategies with automated health checks.
"#;

const LEGACY_WITH_RELATED: &str = r#"---
title: "Database Backup Strategy"
tags: [database, backup]
related: ["infra/disaster-recovery", "ops/runbook-backup"]
importance: 0.9
recency: 0.8
---

PostgreSQL backups run every 6 hours with WAL archiving to S3.
Point-in-time recovery is available up to 7 days.
"#;

// --- Test 13: legacy durable fact compiles ---
#[test]
fn test_legacy_durable_fact_compiles() {
    let tmp = temp_workspace();
    write_fact(tmp.path(), "k8s-autoscaler.md", LEGACY_AUTOSCALER);

    let result = compile_clean(tmp.path());
    assert_eq!(result.index_stats.as_ref().unwrap().documents_written, 1);
    assert_eq!(result.parse_result.error_count, 0);

    let mut cache = ExactCache::new(60);
    let mut fuzzy = FuzzyCache::new(100);
    let r = query_helper(tmp.path(), "Kubernetes Node Autoscaler", &mut cache, &mut fuzzy);
    assert!(!r.hits.is_empty(), "legacy format fact should be queryable by title");
}

// --- Test 14: legacy importance/recency preserved ---
#[test]
fn test_legacy_importance_recency_preserved() {
    let tmp = temp_workspace();

    // Legacy format fact with lower importance/recency
    write_fact(tmp.path(), "k8s-legacy.md", LEGACY_LOW_IMPORTANCE);

    // Engram fact with higher importance/recency, similar content
    write_fact(tmp.path(), "k8s-modern.md",
        r#"---
title: "Kubernetes Modern Setup"
factType: durable
confidence: 1.0
importance: 1.0
recency: 1.0
tags: [infrastructure, kubernetes]
---

Modern Kubernetes cluster setup with autoscaling and GitOps.
"#
    );

    compile_clean(tmp.path());

    let mut cache = ExactCache::new(60);
    let mut fuzzy = FuzzyCache::new(100);
    let r = query_helper(tmp.path(), "kubernetes", &mut cache, &mut fuzzy);

    assert!(r.hits.len() >= 2, "should find both facts");
    // The Engram fact with higher importance/recency should rank above the legacy format fact
    assert!(
        r.hits[0].title.as_deref().unwrap_or("").contains("Modern"),
        "higher importance/recency fact should rank first, got: {:?}",
        r.hits[0].title
    );
}

// --- Test 15: legacy tags searchable ---
#[test]
fn test_legacy_tags_searchable() {
    let tmp = temp_workspace();
    write_fact(tmp.path(), "k8s-autoscaler.md", LEGACY_AUTOSCALER);
    compile_clean(tmp.path());

    let mut cache = ExactCache::new(60);
    let mut fuzzy = FuzzyCache::new(100);
    let r = query_helper(tmp.path(), "kubernetes", &mut cache, &mut fuzzy);
    assert!(!r.hits.is_empty(), "should find fact via tag 'kubernetes'");
}

// --- Test 16: legacy keywords searchable ---
#[test]
fn test_legacy_keywords_searchable() {
    let tmp = temp_workspace();
    write_fact(tmp.path(), "cicd-rollout.md", LEGACY_WITH_KEYWORDS);
    compile_clean(tmp.path());

    let mut cache = ExactCache::new(60);
    let mut fuzzy = FuzzyCache::new(100);
    let r = query_helper(tmp.path(), "rollout deployment", &mut cache, &mut fuzzy);
    assert!(!r.hits.is_empty(), "should find fact via keywords 'rollout deployment'");
}

// --- Test 17: legacy mixed corpus ---
#[test]
fn test_legacy_mixed_corpus() {
    let tmp = temp_workspace();

    // 5 legacy format facts
    write_fact(tmp.path(), "brv-1.md", LEGACY_AUTOSCALER);
    write_fact(tmp.path(), "brv-2.md", LEGACY_NETWORKING);
    write_fact(tmp.path(), "brv-3.md", LEGACY_LOW_IMPORTANCE);
    write_fact(tmp.path(), "brv-4.md", LEGACY_WITH_KEYWORDS);
    write_fact(tmp.path(), "brv-5.md", LEGACY_WITH_RELATED);

    // 5 Engram facts
    write_fact(tmp.path(), "eng-1.md", &durable_fact("Redis Cache Eviction", "LRU eviction policy with maxmemory configuration."));
    write_fact(tmp.path(), "eng-2.md", &durable_fact("PostgreSQL Indexing", "B-tree indexes on frequently queried columns."));
    write_fact(tmp.path(), "eng-3.md", &durable_fact("Terraform State Management", "Remote state in S3 with DynamoDB locking."));
    write_fact(tmp.path(), "eng-4.md", &durable_fact("Docker Image Optimization", "Multi-stage builds reduce final image size."));
    write_fact(tmp.path(), "eng-5.md", &durable_fact("Prometheus Alerting Rules", "Alert on p99 latency exceeding SLO threshold."));

    let result = compile_clean(tmp.path());
    assert_eq!(result.index_stats.as_ref().unwrap().documents_written, 10);

    let mut cache = ExactCache::new(60);
    let mut fuzzy = FuzzyCache::new(100);

    // Query term present only in legacy format facts
    let r = query_helper(tmp.path(), "autoscaler cluster", &mut cache, &mut fuzzy);
    assert!(!r.hits.is_empty(), "legacy-only terms should be queryable");

    cache.invalidate_all();
    fuzzy.invalidate_all();

    // Query term present only in Engram facts
    let r = query_helper(tmp.path(), "Redis cache eviction LRU", &mut cache, &mut fuzzy);
    assert!(!r.hits.is_empty(), "Engram-only terms should be queryable");
}

// --- Test 18: legacy related field preserved ---
#[test]
fn test_legacy_related_field_preserved() {
    let tmp = temp_workspace();
    write_fact(tmp.path(), "db-backup.md", LEGACY_WITH_RELATED);
    compile_clean(tmp.path());

    let mut cache = ExactCache::new(60);
    let mut fuzzy = FuzzyCache::new(100);
    let r = query_helper(tmp.path(), "Database Backup Strategy", &mut cache, &mut fuzzy);

    assert!(!r.hits.is_empty(), "related-field fact should be queryable");
    // The fact should round-trip through the index without corruption
    let hit = &r.hits[0];
    assert!(
        hit.title.as_deref().unwrap_or("").contains("Backup"),
        "title should be preserved, got: {:?}",
        hit.title
    );
}

// --- Test 24: legacy full field parity ---
#[test]
fn test_legacy_full_field_parity() {
    let tmp = temp_workspace();

    let content = r#"---
title: "Full Parity Test Fact"
tags: [parity, test]
keywords: [roundtrip, verification]
importance: 0.75
recency: 0.65
maturity: 0.55
accessCount: 42
updateCount: 7
related: ["other/fact-a", "other/fact-b"]
---

Full parity verification body content.
"#;

    write_fact(tmp.path(), "parity-test.md", content);
    compile_clean(tmp.path());

    let mut cache = ExactCache::new(60);
    let mut fuzzy = FuzzyCache::new(100);
    let r = query_helper(tmp.path(), "parity verification", &mut cache, &mut fuzzy);

    assert!(!r.hits.is_empty(), "parity fact should be found");
    let hit = &r.hits[0];

    // Title round-trips correctly
    assert_eq!(
        hit.title.as_deref(),
        Some("Full Parity Test Fact"),
        "title should match exactly"
    );

    // Importance is the parsed value, not the default
    assert!(
        (hit.importance - 0.75).abs() < f64::EPSILON,
        "importance should be 0.75, got: {}",
        hit.importance
    );

    // Confidence defaults to 1.0 (no confidence field in legacy format)
    assert!(
        (hit.confidence - 1.0).abs() < f64::EPSILON,
        "confidence should default to 1.0, got: {}",
        hit.confidence
    );

    // Recency is the parsed value
    assert!(
        (hit.recency - 0.65).abs() < f64::EPSILON,
        "recency should be 0.65, got: {}",
        hit.recency
    );

    // Fact type defaults to durable (no factType in legacy format)
    assert_eq!(
        hit.fact_type, "durable",
        "fact_type should default to durable"
    );

    // Tags round-trip correctly
    assert!(
        hit.tags.contains(&"parity".to_string()),
        "tags should contain 'parity', got: {:?}",
        hit.tags
    );

    // domain_tags is empty (legacy format has no domain_tags)
    assert!(
        hit.domain_tags.is_empty(),
        "domain_tags should be empty for legacy format facts, got: {:?}",
        hit.domain_tags
    );
}
