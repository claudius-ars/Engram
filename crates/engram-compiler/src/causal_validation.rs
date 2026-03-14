//! Cross-file validation of `caused_by` and `causes` references.
//!
//! Runs after the full manifest is built but before the CSR is written.
//! Operates on `FactRecord` strings only — no knowledge of the CSR file format.

use std::collections::{HashMap, HashSet};

use engram_core::frontmatter::FactRecord;
use engram_core::CausalValidationWarning;

/// Validate all causal references across the full corpus.
///
/// For every fact with non-empty `caused_by` or `causes`, checks each
/// referenced fact_id against the full set of known IDs. Returns warnings
/// for dangling edges, self-loops, and cycles.
///
/// This function does NOT modify the records or remove edges. It only
/// reports problems. The CSR writer consumes these warnings independently.
pub fn validate_causal_references(records: &[FactRecord]) -> Vec<CausalValidationWarning> {
    let mut warnings = Vec::new();

    // Build the set of all known fact IDs
    let known_ids: HashSet<&str> = records.iter().map(|r| r.id.as_str()).collect();

    // Build the canonical edge set: (source_id, target_id)
    // Union of `causes` (forward) and `caused_by` (backward) declarations
    let mut edges: HashSet<(String, String)> = HashSet::new();

    for record in records {
        let source = &record.id;

        let src_path = record.source_path.to_string_lossy().to_string();

        // Forward declarations: this fact causes these targets
        for target in &record.causes {
            if target == source {
                warnings.push(CausalValidationWarning::SelfLoop {
                    fact_id: source.clone(),
                });
                continue;
            }
            if !known_ids.contains(target.as_str()) {
                warnings.push(CausalValidationWarning::DanglingEdge {
                    source_path: src_path.clone(),
                    source_id: source.clone(),
                    target_id: target.clone(),
                });
                continue;
            }
            edges.insert((source.clone(), target.clone()));
        }

        // Backward declarations: these sources caused this fact
        for cause in &record.caused_by {
            if cause == source {
                warnings.push(CausalValidationWarning::SelfLoop {
                    fact_id: source.clone(),
                });
                continue;
            }
            if !known_ids.contains(cause.as_str()) {
                warnings.push(CausalValidationWarning::DanglingEdge {
                    source_path: src_path.clone(),
                    source_id: cause.clone(),
                    target_id: source.clone(),
                });
                continue;
            }
            edges.insert((cause.clone(), source.clone()));
        }
    }

    // Run cycle detection on the edge set
    let cycle_warnings = detect_cycles(&known_ids, &edges);
    warnings.extend(cycle_warnings);

    warnings
}

/// Iterative DFS cycle detection using three-color marking.
///
/// White = unvisited, Gray = in current DFS path, Black = fully explored.
/// When we encounter a gray node, we've found a back-edge (cycle).
fn detect_cycles(
    known_ids: &HashSet<&str>,
    edges: &HashSet<(String, String)>,
) -> Vec<CausalValidationWarning> {
    // Build adjacency list from edge set
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    for (src, tgt) in edges {
        adj.entry(src.as_str()).or_default().push(tgt.as_str());
    }

    // Three-color visited set
    // 0 = white (unvisited), 1 = gray (in path), 2 = black (done)
    let mut color: HashMap<&str, u8> = HashMap::new();
    for id in known_ids {
        color.insert(*id, 0);
    }

    let mut warnings = Vec::new();

    // Track parent pointers for cycle reconstruction
    let mut parent: HashMap<&str, &str> = HashMap::new();

    for &start_id in known_ids {
        if color[start_id] != 0 {
            continue; // already visited
        }

        // Iterative DFS using an explicit stack.
        // Each stack entry is (node, neighbor_index).
        // When we first visit a node, we color it gray.
        // When all neighbors are processed, we color it black.
        let mut stack: Vec<(&str, usize)> = Vec::new();
        *color.get_mut(start_id).unwrap() = 1; // gray
        stack.push((start_id, 0));

        while let Some((node, idx)) = stack.last_mut() {
            let neighbors = adj.get(*node).map(|v| v.as_slice()).unwrap_or(&[]);

            if *idx >= neighbors.len() {
                // All neighbors explored — mark black
                *color.get_mut(*node).unwrap() = 2;
                stack.pop();
                continue;
            }

            let neighbor = neighbors[*idx];
            *idx += 1;

            match color[neighbor] {
                0 => {
                    // White — unvisited, push onto stack
                    *color.get_mut(neighbor).unwrap() = 1;
                    parent.insert(neighbor, *node);
                    stack.push((neighbor, 0));
                }
                1 => {
                    // Gray — back edge, cycle found!
                    // Reconstruct cycle path from the stack
                    let cycle_ids = reconstruct_cycle(&stack, neighbor);
                    if !cycle_ids.is_empty() {
                        warnings.push(CausalValidationWarning::CycleDetected { cycle_ids });
                    }
                }
                _ => {
                    // Black — already fully explored, no cycle through this node
                }
            }
        }
    }

    warnings
}

/// Reconstruct the cycle path from the DFS stack.
///
/// The stack contains the current DFS path. `cycle_target` is the gray node
/// we just found a back-edge to. Walk backward through the stack from the
/// current node to `cycle_target` to extract the cycle.
fn reconstruct_cycle(stack: &[(&str, usize)], cycle_target: &str) -> Vec<String> {
    let mut cycle = Vec::new();

    // Find cycle_target in the stack
    let mut found = false;
    for &(node, _) in stack {
        if node == cycle_target {
            found = true;
        }
        if found {
            cycle.push(node.to_string());
        }
    }

    // Close the cycle by repeating the target
    if !cycle.is_empty() {
        cycle.push(cycle_target.to_string());
    }

    cycle
}

#[cfg(test)]
mod tests {
    use super::*;
    use engram_core::frontmatter::FactRecord;
    use engram_core::FactType;
    use std::path::PathBuf;

    fn make_record(id: &str, caused_by: Vec<&str>, causes: Vec<&str>) -> FactRecord {
        FactRecord {
            id: id.to_string(),
            source_path: PathBuf::from(format!("{}.md", id)),
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

    #[test]
    fn test_no_causal_refs_no_warnings() {
        let records = vec![
            make_record("a", vec![], vec![]),
            make_record("b", vec![], vec![]),
        ];
        let warnings = validate_causal_references(&records);
        assert!(warnings.is_empty());
    }

    #[test]
    fn test_valid_forward_edge() {
        let records = vec![
            make_record("a", vec![], vec!["b"]),
            make_record("b", vec![], vec![]),
        ];
        let warnings = validate_causal_references(&records);
        // No dangling edges, no self-loops, no cycles in a→b
        assert!(
            warnings.is_empty(),
            "valid edge should produce no warnings, got: {:?}",
            warnings
        );
    }

    #[test]
    fn test_valid_backward_edge() {
        let records = vec![
            make_record("a", vec![], vec![]),
            make_record("b", vec!["a"], vec![]),
        ];
        let warnings = validate_causal_references(&records);
        assert!(warnings.is_empty());
    }

    #[test]
    fn test_dangling_forward_edge() {
        let records = vec![make_record("a", vec![], vec!["nonexistent"])];
        let warnings = validate_causal_references(&records);
        assert_eq!(warnings.len(), 1);
        match &warnings[0] {
            CausalValidationWarning::DanglingEdge {
                source_id,
                target_id,
                ..
            } => {
                assert_eq!(source_id, "a");
                assert_eq!(target_id, "nonexistent");
            }
            other => panic!("expected DanglingEdge, got: {:?}", other),
        }
    }

    #[test]
    fn test_dangling_backward_edge() {
        let records = vec![make_record("b", vec!["ghost"], vec![])];
        let warnings = validate_causal_references(&records);
        assert_eq!(warnings.len(), 1);
        match &warnings[0] {
            CausalValidationWarning::DanglingEdge {
                source_id,
                target_id,
                ..
            } => {
                assert_eq!(source_id, "ghost");
                assert_eq!(target_id, "b");
            }
            other => panic!("expected DanglingEdge, got: {:?}", other),
        }
    }

    #[test]
    fn test_self_loop_forward() {
        let records = vec![make_record("a", vec![], vec!["a"])];
        let warnings = validate_causal_references(&records);
        assert!(warnings.iter().any(|w| matches!(
            w,
            CausalValidationWarning::SelfLoop { fact_id } if fact_id == "a"
        )));
    }

    #[test]
    fn test_self_loop_backward() {
        let records = vec![make_record("a", vec!["a"], vec![])];
        let warnings = validate_causal_references(&records);
        assert!(warnings.iter().any(|w| matches!(
            w,
            CausalValidationWarning::SelfLoop { fact_id } if fact_id == "a"
        )));
    }

    #[test]
    fn test_duplicate_edge_from_both_sides() {
        // a.causes = [b] AND b.caused_by = [a] → same edge, no warnings
        let records = vec![
            make_record("a", vec![], vec!["b"]),
            make_record("b", vec!["a"], vec![]),
        ];
        let warnings = validate_causal_references(&records);
        assert!(warnings.is_empty(), "duplicate edge should not warn: {:?}", warnings);
    }

    #[test]
    fn test_simple_cycle_detected() {
        // a → b → a
        let records = vec![
            make_record("a", vec![], vec!["b"]),
            make_record("b", vec![], vec!["a"]),
        ];
        let warnings = validate_causal_references(&records);
        let cycle_warnings: Vec<_> = warnings
            .iter()
            .filter(|w| matches!(w, CausalValidationWarning::CycleDetected { .. }))
            .collect();
        assert!(
            !cycle_warnings.is_empty(),
            "cycle a→b→a should be detected"
        );
    }

    #[test]
    fn test_three_node_cycle() {
        // a → b → c → a
        let records = vec![
            make_record("a", vec![], vec!["b"]),
            make_record("b", vec![], vec!["c"]),
            make_record("c", vec![], vec!["a"]),
        ];
        let warnings = validate_causal_references(&records);
        let cycle_warnings: Vec<_> = warnings
            .iter()
            .filter(|w| matches!(w, CausalValidationWarning::CycleDetected { .. }))
            .collect();
        assert!(
            !cycle_warnings.is_empty(),
            "cycle a→b→c→a should be detected"
        );
    }

    #[test]
    fn test_dag_no_cycle() {
        // a → b → d, a → c → d (diamond, no cycle)
        let records = vec![
            make_record("a", vec![], vec!["b", "c"]),
            make_record("b", vec![], vec!["d"]),
            make_record("c", vec![], vec!["d"]),
            make_record("d", vec![], vec![]),
        ];
        let warnings = validate_causal_references(&records);
        let cycle_warnings: Vec<_> = warnings
            .iter()
            .filter(|w| matches!(w, CausalValidationWarning::CycleDetected { .. }))
            .collect();
        assert!(
            cycle_warnings.is_empty(),
            "diamond DAG should not be detected as cycle: {:?}",
            cycle_warnings
        );
    }

    #[test]
    fn test_mixed_warnings() {
        // a → b (valid), a → ghost (dangling), c → c (self-loop)
        let records = vec![
            make_record("a", vec![], vec!["b", "ghost"]),
            make_record("b", vec![], vec![]),
            make_record("c", vec![], vec!["c"]),
        ];
        let warnings = validate_causal_references(&records);

        let dangling: Vec<_> = warnings
            .iter()
            .filter(|w| matches!(w, CausalValidationWarning::DanglingEdge { .. }))
            .collect();
        let self_loops: Vec<_> = warnings
            .iter()
            .filter(|w| matches!(w, CausalValidationWarning::SelfLoop { .. }))
            .collect();

        assert_eq!(dangling.len(), 1, "should have 1 dangling edge");
        assert_eq!(self_loops.len(), 1, "should have 1 self-loop");
    }

    #[test]
    fn test_empty_corpus() {
        let warnings = validate_causal_references(&[]);
        assert!(warnings.is_empty());
    }

    #[test]
    fn test_disconnected_nodes_no_warnings() {
        let records = vec![
            make_record("x", vec![], vec![]),
            make_record("y", vec![], vec![]),
            make_record("z", vec![], vec![]),
        ];
        let warnings = validate_causal_references(&records);
        assert!(warnings.is_empty());
    }
}
