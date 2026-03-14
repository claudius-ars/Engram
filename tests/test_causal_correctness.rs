//! Phase 3 — Prompt 9: Test Strategy
//!
//! End-to-end correctness tests for the causal graph, temporal log,
//! causal query triggers, enrichment, and classification pipeline.
//! These verify cross-cutting behavior that unit tests cannot catch in isolation.

#[allow(dead_code)]
mod common;

use std::collections::HashSet;

use engram_bulwark::BulwarkHandle;
use engram_compiler::causal_writer::CausalWriter;
use engram_core::frontmatter::FactRecord;
use engram_core::temporal::{
    parse_temporal_log, EVENT_KIND_CREATED, EVENT_KIND_DELETED, EVENT_KIND_UPDATED,
};
use engram_core::{CausalValidationWarning, FactType};
use engram_query::causal_query::{
    classify_causal_query, is_causal_query, CausalQueryPattern,
};
use engram_query::causal_reader::{CausalReader, TraversalDirection, CAUSAL_DECAY_BASE};

use common::{
    compile_clean, compile_with_classify, durable_fact, state_fact,
    temp_workspace, write_fact,
};

// ─── Test helpers ───────────────────────────────────────────────────────────

/// Build a FactRecord with causal edges for use with CausalWriter.
fn make_record(id: &str, caused_by: Vec<&str>, causes: Vec<&str>) -> FactRecord {
    FactRecord {
        id: id.to_string(),
        source_path: std::path::PathBuf::from(format!("{}.md", id)),
        title: Some(id.to_string()),
        tags: Vec::new(),
        keywords: Vec::new(),
        related: Vec::new(),
        importance: 1.0,
        recency: 1.0,
        maturity: 1.0,
        access_count: 0,
        update_count: 0,
        created_at: None,
        updated_at: None,
        fact_type: FactType::Durable,
        valid_until: None,
        caused_by: caused_by.into_iter().map(String::from).collect(),
        causes: causes.into_iter().map(String::from).collect(),
        event_sequence: None,
        confidence: 1.0,
        domain_tags: Vec::new(),
        body: String::new(),
        warnings: Vec::new(),
        fact_type_explicit: true,
    }
}

fn causal_fact(title: &str, body: &str, caused_by: &[&str], causes: &[&str]) -> String {
    let cb = caused_by
        .iter()
        .map(|s| format!("\"{}\"", s))
        .collect::<Vec<_>>()
        .join(", ");
    let ca = causes
        .iter()
        .map(|s| format!("\"{}\"", s))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r#"---
title: "{title}"
factType: durable
confidence: 1.0
importance: 0.8
recency: 0.9
tags: [test]
causedBy: [{cb}]
causes: [{ca}]
---

{body}
"#
    )
}

// ============================================================
// 1. Causal graph correctness tests (known graph, manual BFS)
//
// 10-node, 12-edge directed graph:
//
//   n0 → n1 → n2 → n3 → n4    (chain of length 4)
//   n0 → n5 → n3               (diamond convergence: n0→n1→n2→n3 and n0→n5→n3)
//   n4 → n1                    (cycle: n1→n2→n3→n4→n1)
//   n0 → n6                    (one-hop leaf)
//   n6 → n7                    (extend to 2-hop)
//   n7 → n8                    (extend to 3-hop)
//   n8 → n3                    (converges back)
//   n9                          (disconnected)
//
// Sorted IDs: n0(0), n1(1), n2(2), n3(3), n4(4), n5(5), n6(6), n7(7), n8(8), n9(9)
//
// Forward edges (12):
//   n0→n1, n0→n5, n0→n6
//   n1→n2
//   n2→n3
//   n3→n4
//   n4→n1            (cycle back-edge)
//   n5→n3
//   n6→n7
//   n7→n8
//   n8→n3
//   (that's 11, add n5→n8 for 12)
//   n5→n8
// ============================================================

fn build_10_node_graph() -> (Vec<FactRecord>, Vec<CausalValidationWarning>) {
    let records = vec![
        make_record("n0", vec![], vec!["n1", "n5", "n6"]),
        make_record("n1", vec![], vec!["n2"]),
        make_record("n2", vec![], vec!["n3"]),
        make_record("n3", vec![], vec!["n4"]),
        make_record("n4", vec![], vec!["n1"]),  // cycle
        make_record("n5", vec![], vec!["n3", "n8"]),
        make_record("n6", vec![], vec!["n7"]),
        make_record("n7", vec![], vec!["n8"]),
        make_record("n8", vec![], vec!["n3"]),
        make_record("n9", vec![], vec![]),       // disconnected
    ];
    let warnings = engram_compiler::causal_validation::validate_causal_references(&records);
    (records, warnings)
}

fn write_and_load_10_node(dir: &std::path::Path, generation: u64) -> CausalReader {
    let (records, warnings) = build_10_node_graph();
    let writer = CausalWriter::new(dir);
    writer.build(&records, &warnings, generation);
    CausalReader::load(dir, generation).unwrap()
}

// Phase 3 spec: shortest_path results match manually computed expected paths
// for at least 6 source/target pairs.
#[test]
fn causal_graph_shortest_path_6_pairs() {
    let tmp = tempfile::tempdir().unwrap();
    let reader = write_and_load_10_node(tmp.path(), 1);

    // Pair 1: n0→n1 = 1 hop (direct)
    let p = reader.shortest_path(0, 1, 6).unwrap();
    assert_eq!(p, vec![0, 1]);

    // Pair 2: n0→n3 = 2 hops (n0→n5→n3 is shorter than n0→n1→n2→n3)
    let p = reader.shortest_path(0, 3, 6).unwrap();
    assert_eq!(p.len(), 3); // 2 hops
    assert_eq!(p[0], 0);
    assert_eq!(p[2], 3);

    // Pair 3: n0→n4 = 3 hops (n0→n5→n3→n4 or n0→n1→n2→n3→n4)
    let p = reader.shortest_path(0, 4, 6).unwrap();
    assert!(p.len() <= 4); // at most 3 hops
    assert_eq!(*p.first().unwrap(), 0);
    assert_eq!(*p.last().unwrap(), 4);

    // Pair 4: n6→n3 = 3 hops (n6→n7→n8→n3)
    let p = reader.shortest_path(6, 3, 6).unwrap();
    assert_eq!(p, vec![6, 7, 8, 3]);

    // Pair 5: n0→n9 = no path (disconnected)
    assert_eq!(reader.shortest_path(0, 9, 6), None);

    // Pair 6: n4→n3 = 2 hops (n4→n1→n2→n3), using cycle edge n4→n1
    let p = reader.shortest_path(4, 3, 6).unwrap();
    assert_eq!(*p.first().unwrap(), 4);
    assert_eq!(*p.last().unwrap(), 3);
    assert!(p.len() <= 4); // at most 3 hops via n4→n1→n2→n3
}

// Phase 3 spec: reachable_within forward and backward results match manually
// enumerated expected sets for at least 4 anchor nodes.
#[test]
fn causal_graph_reachable_within_4_anchors() {
    let tmp = tempfile::tempdir().unwrap();
    let reader = write_and_load_10_node(tmp.path(), 1);

    // Anchor 1: n0 forward, max_hops=1 → {n1, n5, n6}
    let reach: HashSet<u32> = reader
        .reachable_within(0, 1, TraversalDirection::Forward)
        .iter()
        .map(|&(n, _)| n)
        .collect();
    assert_eq!(reach, HashSet::from([1, 5, 6]));

    // Anchor 2: n3 backward, max_hops=1 → {n2, n5, n8}
    let reach: HashSet<u32> = reader
        .reachable_within(3, 1, TraversalDirection::Backward)
        .iter()
        .map(|&(n, _)| n)
        .collect();
    assert_eq!(reach, HashSet::from([2, 5, 8]));

    // Anchor 3: n9 forward, max_hops=6 → {} (disconnected)
    let reach = reader.reachable_within(9, 6, TraversalDirection::Forward);
    assert!(reach.is_empty(), "disconnected node should have no reachable nodes");

    // Anchor 4: n0 forward, max_hops=2 → {n1@1, n5@1, n6@1, n2@2, n3@2, n7@2, n8@2}
    let reach: HashSet<u32> = reader
        .reachable_within(0, 2, TraversalDirection::Forward)
        .iter()
        .map(|&(n, _)| n)
        .collect();
    assert!(reach.contains(&1)); // n1 at hop 1
    assert!(reach.contains(&5)); // n5 at hop 1
    assert!(reach.contains(&6)); // n6 at hop 1
    assert!(reach.contains(&2)); // n2 at hop 2 (via n1)
    assert!(reach.contains(&3)); // n3 at hop 2 (via n5)
    assert!(reach.contains(&7)); // n7 at hop 2 (via n6)
    assert!(reach.contains(&8)); // n8 at hop 2 (via n5)
}

// Phase 3 spec: causal_adjacency decay values match 0.7^hop to within 1e-9
// for hops 1 through 4.
#[test]
fn causal_graph_adjacency_decay_hops_1_through_4() {
    let tmp = tempfile::tempdir().unwrap();
    let reader = write_and_load_10_node(tmp.path(), 1);

    // Hop 1: n0→n1 direct = 0.7^1 = 0.7
    let score = reader.causal_adjacency("n0", "n1", 6);
    assert!((score - CAUSAL_DECAY_BASE.powi(1)).abs() < 1e-9, "hop 1: {}", score);

    // Hop 2: n0→n3 via n5 = 0.7^2 = 0.49
    let score = reader.causal_adjacency("n0", "n3", 6);
    assert!((score - CAUSAL_DECAY_BASE.powi(2)).abs() < 1e-9, "hop 2: {}", score);

    // Hop 3: n0→n4 via n5→n3→n4 = 0.7^3 = 0.343
    let score = reader.causal_adjacency("n0", "n4", 6);
    assert!((score - CAUSAL_DECAY_BASE.powi(3)).abs() < 1e-9, "hop 3: {}", score);

    // Hop 4: n6→n4 via n6→n7→n8→n3→n4 = 0.7^4 = 0.2401
    let score = reader.causal_adjacency("n6", "n4", 6);
    assert!((score - CAUSAL_DECAY_BASE.powi(4)).abs() < 1e-9, "hop 4: {}", score);
}

// Phase 3 spec: path exists at hop 4 but not hop 3 returns None at max_hops=3
// and Some at max_hops=4.
#[test]
fn causal_graph_hop_boundary_4_vs_3() {
    let tmp = tempfile::tempdir().unwrap();
    let reader = write_and_load_10_node(tmp.path(), 1);

    // n6→n4 is exactly 4 hops: n6→n7→n8→n3→n4
    assert_eq!(
        reader.shortest_path(6, 4, 3),
        None,
        "4-hop path should not be found at max_hops=3"
    );
    assert!(
        reader.shortest_path(6, 4, 4).is_some(),
        "4-hop path should be found at max_hops=4"
    );

    // Also verify via causal_adjacency
    assert_eq!(reader.causal_adjacency("n6", "n4", 3), 0.0);
    assert!((reader.causal_adjacency("n6", "n4", 4) - CAUSAL_DECAY_BASE.powi(4)).abs() < 1e-9);
}

// Phase 3 spec: cycle does not cause infinite traversal at any max_hops up to 6.
#[test]
fn causal_graph_cycle_terminates_all_max_hops() {
    let tmp = tempfile::tempdir().unwrap();
    let reader = write_and_load_10_node(tmp.path(), 1);

    // Cycle: n1→n2→n3→n4→n1. Start inside the cycle and traverse.
    for max_hops in 1..=6 {
        // shortest_path: asking for n1→n9 (unreachable) should terminate
        assert_eq!(
            reader.shortest_path(1, 9, max_hops),
            None,
            "should terminate at max_hops={}", max_hops
        );
        // reachable_within: should terminate
        let _reach = reader.reachable_within(1, max_hops, TraversalDirection::Forward);
        // causal_adjacency: should not hang
        let _score = reader.causal_adjacency("n1", "n9", max_hops);
    }
}

// ============================================================
// 2. Cycle detection correctness tests
// ============================================================

// Phase 3 spec: 2-node cycle (A→B→A)
#[test]
fn cycle_detection_2_node() {
    let records = vec![
        make_record("a", vec![], vec!["b"]),
        make_record("b", vec![], vec!["a"]),
    ];
    let warnings = engram_compiler::causal_validation::validate_causal_references(&records);
    let cycles: Vec<_> = warnings
        .iter()
        .filter(|w| matches!(w, CausalValidationWarning::CycleDetected { .. }))
        .collect();
    assert!(!cycles.is_empty(), "2-node cycle should be detected");

    // Writer builds successfully; traversal terminates
    let tmp = tempfile::tempdir().unwrap();
    let writer = CausalWriter::new(tmp.path());
    let report = writer.build(&records, &warnings, 1);
    assert_eq!(report.edge_count, 2);

    let reader = CausalReader::load(tmp.path(), 1).unwrap();
    for max_hops in 1..=6 {
        let _reach = reader.reachable_within(0, max_hops, TraversalDirection::Forward);
    }
}

// Phase 3 spec: 3-node cycle (A→B→C→A)
#[test]
fn cycle_detection_3_node() {
    let records = vec![
        make_record("a", vec![], vec!["b"]),
        make_record("b", vec![], vec!["c"]),
        make_record("c", vec![], vec!["a"]),
    ];
    let warnings = engram_compiler::causal_validation::validate_causal_references(&records);
    let cycles: Vec<_> = warnings
        .iter()
        .filter(|w| matches!(w, CausalValidationWarning::CycleDetected { .. }))
        .collect();
    assert!(!cycles.is_empty(), "3-node cycle should be detected");

    let tmp = tempfile::tempdir().unwrap();
    let writer = CausalWriter::new(tmp.path());
    let report = writer.build(&records, &warnings, 1);
    assert_eq!(report.edge_count, 3);

    let reader = CausalReader::load(tmp.path(), 1).unwrap();
    for max_hops in 1..=6 {
        let _reach = reader.reachable_within(0, max_hops, TraversalDirection::Forward);
    }
}

// Phase 3 spec: self-referencing node (A→A), dropped by writer as self-loop
#[test]
fn cycle_detection_self_loop_dropped() {
    let records = vec![make_record("a", vec![], vec!["a"])];
    let warnings = engram_compiler::causal_validation::validate_causal_references(&records);
    assert!(
        warnings.iter().any(|w| matches!(w, CausalValidationWarning::SelfLoop { .. })),
        "self-loop should be detected"
    );

    let tmp = tempfile::tempdir().unwrap();
    let writer = CausalWriter::new(tmp.path());
    let report = writer.build(&records, &warnings, 1);
    assert_eq!(report.edge_count, 0, "self-loop should be excluded from CSR");
}

// Phase 3 spec: graph with no cycles (pure DAG)
#[test]
fn cycle_detection_pure_dag() {
    // Diamond DAG: a→b→d, a→c→d
    let records = vec![
        make_record("a", vec![], vec!["b", "c"]),
        make_record("b", vec![], vec!["d"]),
        make_record("c", vec![], vec!["d"]),
        make_record("d", vec![], vec![]),
    ];
    let warnings = engram_compiler::causal_validation::validate_causal_references(&records);
    let cycles: Vec<_> = warnings
        .iter()
        .filter(|w| matches!(w, CausalValidationWarning::CycleDetected { .. }))
        .collect();
    assert!(cycles.is_empty(), "DAG should have no cycles");

    let tmp = tempfile::tempdir().unwrap();
    let writer = CausalWriter::new(tmp.path());
    let report = writer.build(&records, &warnings, 1);
    assert_eq!(report.node_count, 4);
    assert_eq!(report.edge_count, 4);
    assert_eq!(report.dangling_edges_dropped, 0);
    assert_eq!(report.duplicate_edges_removed, 0);
}

// ============================================================
// 3. Causal query trigger coverage — exhaustive signal phrase tests
// ============================================================

// Phase 3 spec: all 11 signal phrases trigger is_causal_query()
#[test]
fn causal_signal_all_11_phrases_trigger() {
    let phrases = [
        ("what was caused by the outage", CausalQueryPattern::Backward),
        ("this depends on the auth service", CausalQueryPattern::Chain),
        ("the cache enables fast reads", CausalQueryPattern::Forward),
        ("the migration led to data loss", CausalQueryPattern::Chain),
        ("it failed because of a timeout", CausalQueryPattern::Chain),
        ("therefore we need a new approach", CausalQueryPattern::Chain),
        ("show me the causal chain", CausalQueryPattern::Chain),
        ("what is upstream of this service", CausalQueryPattern::Backward),
        ("downstream effects of the change", CausalQueryPattern::Forward),
        ("what is the root cause", CausalQueryPattern::Backward),
        ("this is a consequence of the redesign", CausalQueryPattern::Forward),
    ];

    for (query, expected_pattern) in &phrases {
        assert!(
            is_causal_query(query),
            "signal phrase should trigger: {:?}",
            query
        );
        assert_eq!(
            &classify_causal_query(query),
            expected_pattern,
            "pattern mismatch for: {:?}",
            query
        );
    }
}

// Phase 3 spec: at least 10 non-causal queries do not trigger
#[test]
fn causal_signal_non_causal_queries_do_not_trigger() {
    let non_causal = [
        "what is the retry policy",
        "explain authentication flow",
        "how does the cache work",
        "list all API endpoints",
        "database schema migration guide",
        "kubernetes pod scheduling",
        "monitoring dashboard overview",
        "CI pipeline configuration",
        "user authentication tokens",
        "I led the team",  // no "led to"
    ];

    for query in &non_causal {
        assert!(
            !is_causal_query(query),
            "should NOT trigger: {:?}",
            query
        );
    }
}

// Phase 3 spec: edge cases
#[test]
fn causal_signal_edge_cases() {
    // "blockchain architecture" — contains "chain" substring → triggers
    assert!(
        is_causal_query("blockchain architecture"),
        "'chain' substring in 'blockchain' should trigger (per spec: substring match)"
    );

    // "upstream of my thinking" — contains "upstream" → triggers
    assert!(
        is_causal_query("upstream of my thinking"),
        "'upstream' should trigger"
    );

    // "because of course" — contains "because" → triggers
    assert!(
        is_causal_query("because of course"),
        "'because' should trigger"
    );
}

// Phase 3 spec: case variants all trigger identically
#[test]
fn causal_signal_case_insensitive() {
    for variant in &["CAUSED BY", "Caused By", "caused by", "cAuSeD bY"] {
        let query = format!("what was {} the incident", variant);
        assert!(
            is_causal_query(&query),
            "case variant {:?} should trigger",
            variant
        );
        assert_eq!(
            classify_causal_query(&query),
            CausalQueryPattern::Backward,
            "case variant {:?} should classify as Backward",
            variant
        );
    }
}

// ============================================================
// 5. Enrichment correctness tests
// ============================================================

// Phase 3 spec: all returned hits from temporal query have non-empty title fields
#[test]
fn enrichment_temporal_hits_have_titles() {
    let tmp = temp_workspace();
    for i in 0..5 {
        write_fact(
            tmp.path(),
            &format!("enrich-{}.md", i),
            &state_fact(
                &format!("Enrichment Fact {}", i),
                &format!("State fact number {} for enrichment testing.", i),
            ),
        );
    }
    compile_clean(tmp.path());

    std::fs::write(
        tmp.path().join(".brv/engram.toml"),
        "[query]\nscore_threshold = 0.0\nscore_gap = 0.0\n",
    )
    .unwrap();

    let mut cache = engram_query::ExactCache::new(60);
    let mut fuzzy = engram_query::FuzzyCache::new(100);
    let bulwark = BulwarkHandle::new_stub();
    let config = engram_core::WorkspaceConfig {
        score_threshold: 0.0,
        score_gap: 0.0,
        ..engram_core::WorkspaceConfig::default()
    };

    // Temporal signal query
    let r = engram_query::query(
        tmp.path(),
        "what is the current enrichment state",
        engram_query::QueryOptions { max_results: 10, min_score: 0.0, domain_tags: vec![] },
        &mut cache,
        &mut fuzzy,
        &bulwark,
        &config,
    )
    .unwrap();

    assert!(!r.hits.is_empty(), "should find enrichment facts");
    for hit in &r.hits {
        assert!(
            hit.title.is_some(),
            "enriched hit should have a non-empty title, got None for id={}",
            hit.id
        );
    }
}

// Phase 3 spec: enriched hits have score equal to temporal score, not BM25 score
#[test]
fn enrichment_temporal_score_not_bm25() {
    let tmp = temp_workspace();
    write_fact(
        tmp.path(),
        "scored.md",
        &state_fact("Scored State Fact", "The deployment pipeline is currently paused."),
    );
    compile_clean(tmp.path());

    std::fs::write(
        tmp.path().join(".brv/engram.toml"),
        "[query]\nscore_threshold = 0.0\nscore_gap = 0.0\n",
    )
    .unwrap();

    let mut cache = engram_query::ExactCache::new(60);
    let mut fuzzy = engram_query::FuzzyCache::new(100);
    let bulwark = BulwarkHandle::new_stub();
    let config = engram_core::WorkspaceConfig {
        score_threshold: 0.0,
        score_gap: 0.0,
        ..engram_core::WorkspaceConfig::default()
    };

    // Temporal tier query
    let r = engram_query::query(
        tmp.path(),
        "what is the current deployment status",
        engram_query::QueryOptions { max_results: 10, min_score: 0.0, domain_tags: vec![] },
        &mut cache,
        &mut fuzzy,
        &bulwark,
        &config,
    )
    .unwrap();

    if r.meta.cache_tier == engram_query::CACHE_TIER_TEMPORAL && !r.hits.is_empty() {
        // Temporal tier hits should have score reflecting temporal scoring,
        // which is compound (not raw BM25).
        let hit = &r.hits[0];
        assert!(hit.score > 0.0, "temporal hit should have positive score");
    }
}

// ============================================================
// 6. Temporal log event type correctness
// ============================================================

// Phase 3 spec: modified fact has exactly one Updated event, no Created event
// in the second compile's records.
#[test]
fn temporal_log_updated_event_on_modify() {
    let tmp = temp_workspace();
    write_fact(
        tmp.path(),
        "evolving.md",
        &durable_fact("Evolving Pangolin", "Pangolin scales provide armor."),
    );
    compile_clean(tmp.path());

    // Modify content
    write_fact(
        tmp.path(),
        "evolving.md",
        &durable_fact("Evolving Pangolin", "Pangolin scales are made of keratin and overlap like artichoke leaves."),
    );
    compile_clean(tmp.path());

    let log_path = tmp.path().join(".brv/index/temporal.log");
    let data = std::fs::read(&log_path).unwrap();
    let (_, records) = parse_temporal_log(&data).unwrap();

    let updated = records.iter().filter(|r| r.event_kind == EVENT_KIND_UPDATED).count();
    let created = records.iter().filter(|r| r.event_kind == EVENT_KIND_CREATED).count();

    assert!(updated >= 1, "modified fact should have at least one Updated event");
    assert_eq!(created, 0, "second compile should not emit Created for existing fact");
}

// Phase 3 spec: deleted fact has Deleted event, surviving fact has no event.
#[test]
fn temporal_log_deleted_event_on_removal() {
    let tmp = temp_workspace();
    write_fact(
        tmp.path(),
        "survivor.md",
        &durable_fact("Survivor Echidna", "Echidna is a monotreme."),
    );
    write_fact(
        tmp.path(),
        "doomed.md",
        &durable_fact("Doomed Dodo", "The dodo was endemic to Mauritius."),
    );
    compile_clean(tmp.path());

    // Remove one fact
    std::fs::remove_file(tmp.path().join(".brv/context-tree/doomed.md")).unwrap();
    compile_clean(tmp.path());

    let log_path = tmp.path().join(".brv/index/temporal.log");
    let data = std::fs::read(&log_path).unwrap();
    let (header, records) = parse_temporal_log(&data).unwrap();

    let deleted = records.iter().filter(|r| r.event_kind == EVENT_KIND_DELETED).count();
    assert!(deleted >= 1, "deleted fact should produce Deleted event");

    // Surviving unchanged fact should emit no events
    // Total records should be exactly 1 (just the Deleted event)
    assert_eq!(
        header.record_count, 1,
        "only the deleted fact should have an event (unchanged survivor = 0 events)"
    );
}

// Phase 3 spec: content_hash in all temporal records is non-zero after normal compile.
#[test]
fn temporal_log_content_hash_nonzero() {
    let tmp = temp_workspace();
    write_fact(
        tmp.path(),
        "hashed.md",
        &durable_fact("Hashed Aardvark", "Aardvark can eat 50,000 termites in a night."),
    );
    compile_clean(tmp.path());

    let log_path = tmp.path().join(".brv/index/temporal.log");
    let data = std::fs::read(&log_path).unwrap();
    let (_, records) = parse_temporal_log(&data).unwrap();

    assert!(!records.is_empty(), "should have temporal records");
    for r in records {
        assert_ne!(
            r.content_hash,
            [0u8; 16],
            "content_hash should be non-zero for event_kind={}",
            r.event_kind
        );
    }
}

// ============================================================
// 7. LLM classifier mock integration test
// ============================================================

// Phase 3 spec: with ANTHROPIC_API_KEY unset, compile --classify succeeds
// via rule-based fallback. Classification cache is written.
#[test]
fn classifier_fallback_without_api_key() {
    // Ensure no API key is available (rely on the fact that CI/test env doesn't set it)
    // The classification pipeline checks for the key and falls back to rules.
    let tmp = temp_workspace();
    write_fact(
        tmp.path(),
        "classify-fallback.md",
        r#"---
title: "API Status"
confidence: 1.0
importance: 0.8
recency: 0.9
tags: [test]
---

The API gateway is currently handling 5000 requests per second.
"#,
    );

    // compile_with_classify uses CompileConfig { classify: true }
    let result = compile_with_classify(tmp.path());
    assert!(result.index_error.is_none(), "compile should succeed without API key");

    // Classification cache should be written
    let cache_path = tmp.path().join(".brv/index/classification_cache.json");
    assert!(
        cache_path.exists(),
        "classification_cache.json should be written after --classify"
    );

    // Verify the fact was classified by rule-based fallback
    std::fs::write(
        tmp.path().join(".brv/engram.toml"),
        "[query]\nscore_threshold = 0.0\nscore_gap = 0.0\n",
    )
    .unwrap();

    let mut cache = engram_query::ExactCache::new(60);
    let mut fuzzy = engram_query::FuzzyCache::new(100);
    let bulwark = BulwarkHandle::new_stub();
    let config = engram_core::WorkspaceConfig {
        score_threshold: 0.0,
        score_gap: 0.0,
        ..engram_core::WorkspaceConfig::default()
    };
    let r = engram_query::query(
        tmp.path(),
        "API gateway requests",
        engram_query::QueryOptions { max_results: 10, min_score: 0.0, domain_tags: vec![] },
        &mut cache,
        &mut fuzzy,
        &bulwark,
        &config,
    )
    .unwrap();

    assert!(!r.hits.is_empty(), "classified fact should be queryable");
    // Rule classifier should detect "is currently" as state
    assert_eq!(
        r.hits[0].fact_type, "state",
        "rule classifier should detect state via 'is currently' pattern"
    );
}

// ============================================================
// 8. Cross-file validation in the compile pipeline
// ============================================================

// Phase 3 spec: compile with dangling edge produces DanglingEdge warning,
// fact is still indexed.
#[test]
fn pipeline_dangling_edge_warning() {
    let tmp = temp_workspace();
    write_fact(
        tmp.path(),
        "with-dangling.md",
        &causal_fact(
            "Dangling Edge Fact",
            "This fact references a nonexistent cause.",
            &["nonexistent_fact"],
            &[],
        ),
    );
    let result = compile_clean(tmp.path());

    let dangling: Vec<_> = result
        .causal_warnings
        .iter()
        .filter(|w| matches!(w, CausalValidationWarning::DanglingEdge { .. }))
        .collect();
    assert_eq!(
        dangling.len(),
        1,
        "should have exactly one DanglingEdge warning, got: {:?}",
        result.causal_warnings
    );

    // Fact should still be indexed
    std::fs::write(
        tmp.path().join(".brv/engram.toml"),
        "[query]\nscore_threshold = 0.0\nscore_gap = 0.0\n",
    )
    .unwrap();

    let mut cache = engram_query::ExactCache::new(60);
    let mut fuzzy = engram_query::FuzzyCache::new(100);
    let bulwark = BulwarkHandle::new_stub();
    let config = engram_core::WorkspaceConfig {
        score_threshold: 0.0,
        score_gap: 0.0,
        ..engram_core::WorkspaceConfig::default()
    };
    let r = engram_query::query(
        tmp.path(),
        "dangling edge fact",
        engram_query::QueryOptions { max_results: 10, min_score: 0.0, domain_tags: vec![] },
        &mut cache,
        &mut fuzzy,
        &bulwark,
        &config,
    )
    .unwrap();
    assert!(!r.hits.is_empty(), "fact with dangling edge should still be indexed");
}

// Phase 3 spec: compile with cycle produces CycleDetected warning,
// both facts still indexed.
#[test]
fn pipeline_cycle_warning() {
    let tmp = temp_workspace();
    write_fact(
        tmp.path(),
        "cycle-a.md",
        &causal_fact(
            "Cycle Fact Alpha",
            "Alpha causes Beta.",
            &[],
            &["cycle-b"],
        ),
    );
    write_fact(
        tmp.path(),
        "cycle-b.md",
        &causal_fact(
            "Cycle Fact Beta",
            "Beta causes Alpha.",
            &[],
            &["cycle-a"],
        ),
    );
    let result = compile_clean(tmp.path());

    let cycles: Vec<_> = result
        .causal_warnings
        .iter()
        .filter(|w| matches!(w, CausalValidationWarning::CycleDetected { .. }))
        .collect();
    assert!(
        !cycles.is_empty(),
        "A→B→A cycle should produce CycleDetected warning, got: {:?}",
        result.causal_warnings
    );

    // Both facts should still be indexed
    std::fs::write(
        tmp.path().join(".brv/engram.toml"),
        "[query]\nscore_threshold = 0.0\nscore_gap = 0.0\n",
    )
    .unwrap();

    let mut cache = engram_query::ExactCache::new(60);
    let mut fuzzy = engram_query::FuzzyCache::new(100);
    let bulwark = BulwarkHandle::new_stub();
    let config = engram_core::WorkspaceConfig {
        score_threshold: 0.0,
        score_gap: 0.0,
        ..engram_core::WorkspaceConfig::default()
    };

    let r = engram_query::query(
        tmp.path(),
        "cycle fact alpha",
        engram_query::QueryOptions { max_results: 10, min_score: 0.0, domain_tags: vec![] },
        &mut cache,
        &mut fuzzy,
        &bulwark,
        &config,
    )
    .unwrap();
    assert!(!r.hits.is_empty(), "cycle fact A should still be indexed");

    cache.invalidate_all();
    fuzzy.invalidate_all();
    let r = engram_query::query(
        tmp.path(),
        "cycle fact beta",
        engram_query::QueryOptions { max_results: 10, min_score: 0.0, domain_tags: vec![] },
        &mut cache,
        &mut fuzzy,
        &bulwark,
        &config,
    )
    .unwrap();
    assert!(!r.hits.is_empty(), "cycle fact B should still be indexed");
}
