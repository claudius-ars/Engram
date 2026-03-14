//! CSR writer for the causal graph.
//!
//! Takes the validated `FactRecord` slice and `CausalValidationWarning`s
//! produced by `validate_causal_references()`, builds the canonical edge set,
//! assigns lexicographic node indices, constructs bidirectional CSR arrays,
//! and writes `.brv/index/causal.csr` atomically (`.tmp` + rename).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use bytemuck;
use engram_core::frontmatter::FactRecord;
use engram_core::{
    align_up, quantize_weight, CausalBuildReport, CausalEdge, CausalHeader, CausalNode,
    CausalValidationWarning, CAUSAL_MAGIC, CAUSAL_VERSION,
};

/// Writer for the causal graph CSR file.
pub struct CausalWriter {
    index_dir: PathBuf,
}

impl CausalWriter {
    pub fn new(index_dir: &Path) -> Self {
        CausalWriter {
            index_dir: index_dir.to_path_buf(),
        }
    }

    /// Build and write the causal graph CSR file.
    ///
    /// `records` — the full set of parsed facts (same slice passed to validation).
    /// `warnings` — the warnings already produced by `validate_causal_references()`.
    /// `generation` — the current index state generation.
    ///
    /// Returns a `CausalBuildReport` summarizing the build.
    pub fn build(
        &self,
        records: &[FactRecord],
        warnings: &[CausalValidationWarning],
        generation: u64,
    ) -> CausalBuildReport {
        let build_start = std::time::Instant::now();

        // ── Phase 1: Collect canonical edge set ─────────────────────────
        // Mirror the validation logic but only collect valid edges.
        // Count dangling and self-loop edges for the report.
        let known_ids: HashSet<&str> = records.iter().map(|r| r.id.as_str()).collect();

        let mut edges: HashSet<(String, String)> = HashSet::new();
        let mut total_declarations: usize = 0;
        let mut dangling_count: u32 = 0;
        let mut self_loop_count: u32 = 0;

        for record in records {
            let source = &record.id;

            for target in &record.causes {
                total_declarations += 1;
                if target == source {
                    self_loop_count += 1;
                    continue;
                }
                if !known_ids.contains(target.as_str()) {
                    dangling_count += 1;
                    continue;
                }
                edges.insert((source.clone(), target.clone()));
            }

            for cause in &record.caused_by {
                total_declarations += 1;
                if cause == source {
                    self_loop_count += 1;
                    continue;
                }
                if !known_ids.contains(cause.as_str()) {
                    dangling_count += 1;
                    continue;
                }
                edges.insert((cause.clone(), source.clone()));
            }
        }

        let duplicate_edges_removed =
            (total_declarations as u32).saturating_sub(edges.len() as u32 + dangling_count + self_loop_count);

        // ── Phase 2: Compute graph fingerprint ──────────────────────────
        let graph_fingerprint = compute_fingerprint(&edges);

        // ── Phase 3: Incremental gate — check existing file ─────────────
        let csr_path = self.index_dir.join("causal.csr");
        if csr_path.exists() {
            if let Some(existing_header) = read_existing_header(&csr_path) {
                if existing_header.graph_fingerprint == graph_fingerprint
                    && existing_header.generation == generation
                {
                    let build_ms = build_start.elapsed().as_millis() as u64;
                    return CausalBuildReport {
                        node_count: existing_header.node_count,
                        edge_count: existing_header.edge_count,
                        dangling_edges_dropped: dangling_count,
                        duplicate_edges_removed,
                        build_ms,
                        graph_fingerprint,
                        skipped_unchanged: true,
                        warnings: warnings.to_vec(),
                    };
                }
            }
        }

        // ── Phase 4: Assign lexicographic node indices ──────────────────
        let mut sorted_ids: Vec<&str> = known_ids.into_iter().collect();
        sorted_ids.sort_unstable();

        let id_to_index: HashMap<&str, u32> = sorted_ids
            .iter()
            .enumerate()
            .map(|(i, id)| (*id, i as u32))
            .collect();

        // Build a map from fact_id to record for metadata lookup
        let id_to_record: HashMap<&str, &FactRecord> =
            records.iter().map(|r| (r.id.as_str(), r)).collect();

        let node_count = sorted_ids.len() as u32;
        let edge_count = edges.len() as u32;

        // ── Phase 5: Build string table and node table ──────────────────
        let mut string_table = Vec::new();
        let mut nodes = Vec::with_capacity(sorted_ids.len());

        for &fact_id in &sorted_ids {
            let id_offset = string_table.len() as u32;
            let id_bytes = fact_id.as_bytes();
            string_table.extend_from_slice(id_bytes);
            let id_len = id_bytes.len() as u32;

            let record = id_to_record[fact_id];
            let fact_type = match record.fact_type {
                engram_core::FactType::Durable => 0u8,
                engram_core::FactType::State => 1u8,
                engram_core::FactType::Event => 2u8,
            };

            let source_path_hash = engram_core::temporal::fnv1a_64(
                record.source_path.to_string_lossy().as_bytes(),
            );

            nodes.push(CausalNode {
                id_offset,
                id_len,
                source_path_hash,
                importance_bits: record.importance.to_bits(),
                fact_type,
                out_degree: 0, // filled below
                in_degree: 0,  // filled below
                _pad: [0; 5],
            });
        }

        // ── Phase 6: Build forward CSR ──────────────────────────────────
        // For each node, collect outgoing edges sorted by target index.
        let mut fwd_adj: Vec<Vec<CausalEdge>> = vec![Vec::new(); sorted_ids.len()];

        for (src_id, tgt_id) in &edges {
            let src_idx = id_to_index[src_id.as_str()];
            let tgt_idx = id_to_index[tgt_id.as_str()];
            let src_record = id_to_record[src_id.as_str()];
            let weight = quantize_weight(src_record.confidence * src_record.importance);

            fwd_adj[src_idx as usize].push(CausalEdge {
                target_node: tgt_idx,
                weight,
                _pad: [0; 2],
            });
        }

        // Sort each adjacency list by target_node for deterministic output
        for adj in &mut fwd_adj {
            adj.sort_by_key(|e| e.target_node);
        }

        // Build forward offsets array
        let mut fwd_offsets: Vec<u32> = Vec::with_capacity(sorted_ids.len() + 1);
        let mut running = 0u32;
        for (i, adj) in fwd_adj.iter().enumerate() {
            fwd_offsets.push(running);
            let degree = adj.len().min(255) as u8;
            nodes[i].out_degree = degree;
            running += adj.len() as u32;
        }
        fwd_offsets.push(running);

        let fwd_targets: Vec<CausalEdge> = fwd_adj.into_iter().flatten().collect();

        // ── Phase 7: Build backward CSR (transpose) ─────────────────────
        let mut bwd_adj: Vec<Vec<CausalEdge>> = vec![Vec::new(); sorted_ids.len()];

        for (src_id, tgt_id) in &edges {
            let src_idx = id_to_index[src_id.as_str()];
            let tgt_idx = id_to_index[tgt_id.as_str()];
            let src_record = id_to_record[src_id.as_str()];
            let weight = quantize_weight(src_record.confidence * src_record.importance);

            // Backward: index by target, point back to source
            bwd_adj[tgt_idx as usize].push(CausalEdge {
                target_node: src_idx,
                weight,
                _pad: [0; 2],
            });
        }

        for adj in &mut bwd_adj {
            adj.sort_by_key(|e| e.target_node);
        }

        let mut bwd_offsets: Vec<u32> = Vec::with_capacity(sorted_ids.len() + 1);
        let mut running = 0u32;
        for (i, adj) in bwd_adj.iter().enumerate() {
            bwd_offsets.push(running);
            let degree = adj.len().min(255) as u8;
            nodes[i].in_degree = degree;
            running += adj.len() as u32;
        }
        bwd_offsets.push(running);

        let bwd_targets: Vec<CausalEdge> = bwd_adj.into_iter().flatten().collect();

        // ── Phase 8: Assemble header ────────────────────────────────────
        let string_table_bytes = string_table.len() as u32;

        let header = CausalHeader {
            magic: CAUSAL_MAGIC,
            version: CAUSAL_VERSION,
            node_count,
            edge_count,
            string_table_bytes,
            generation,
            graph_fingerprint,
            _pad: [0; 16],
        };

        // ── Phase 9: Serialize to bytes ─────────────────────────────────
        let total_size = engram_core::expected_file_size(&header);
        let mut buf = Vec::with_capacity(total_size);

        // Header (64 bytes)
        buf.extend_from_slice(bytemuck::bytes_of(&header));

        // String table + alignment padding
        buf.extend_from_slice(&string_table);
        let padded_st = align_up(string_table.len(), 4);
        buf.resize(64 + padded_st, 0);

        // Node table
        for node in &nodes {
            buf.extend_from_slice(bytemuck::bytes_of(node));
        }

        // Forward CSR offsets
        for &off in &fwd_offsets {
            buf.extend_from_slice(&off.to_le_bytes());
        }

        // Forward CSR targets
        for edge in &fwd_targets {
            buf.extend_from_slice(bytemuck::bytes_of(edge));
        }

        // Backward CSR offsets
        for &off in &bwd_offsets {
            buf.extend_from_slice(&off.to_le_bytes());
        }

        // Backward CSR targets
        for edge in &bwd_targets {
            buf.extend_from_slice(bytemuck::bytes_of(edge));
        }

        debug_assert_eq!(buf.len(), total_size);

        // ── Phase 10: Atomic write ──────────────────────────────────────
        let _ = std::fs::create_dir_all(&self.index_dir);
        let tmp_path = self.index_dir.join("causal.csr.tmp");

        if let Err(e) = std::fs::write(&tmp_path, &buf) {
            eprintln!("WARN: failed to write {}: {}", tmp_path.display(), e);
        } else if let Err(e) = std::fs::rename(&tmp_path, &csr_path) {
            eprintln!("WARN: failed to rename causal.csr.tmp → causal.csr: {}", e);
        }

        let build_ms = build_start.elapsed().as_millis() as u64;

        CausalBuildReport {
            node_count,
            edge_count,
            dangling_edges_dropped: dangling_count,
            duplicate_edges_removed,
            build_ms,
            graph_fingerprint,
            skipped_unchanged: false,
            warnings: warnings.to_vec(),
        }
    }
}

/// Compute the graph fingerprint: BLAKE3 over sorted "(source\0target\n)" pairs,
/// truncated to 128 bits.
fn compute_fingerprint(edges: &HashSet<(String, String)>) -> [u8; 16] {
    let mut sorted_edges: Vec<(&str, &str)> = edges.iter().map(|(s, t)| (s.as_str(), t.as_str())).collect();
    sorted_edges.sort_unstable();

    let mut hasher = blake3::Hasher::new();
    for (src, tgt) in &sorted_edges {
        hasher.update(src.as_bytes());
        hasher.update(b"\0");
        hasher.update(tgt.as_bytes());
        hasher.update(b"\n");
    }

    let hash = hasher.finalize();
    let mut fingerprint = [0u8; 16];
    fingerprint.copy_from_slice(&hash.as_bytes()[..16]);
    fingerprint
}

/// Read just the 64-byte header from an existing causal.csr file.
/// Returns `None` if the file can't be read or the header is invalid.
fn read_existing_header(path: &Path) -> Option<CausalHeader> {
    let data = std::fs::read(path).ok()?;
    if data.len() < 64 {
        return None;
    }
    let header: &CausalHeader = bytemuck::from_bytes(&data[..64]);
    if engram_core::validate_causal_header(header).is_err() {
        return None;
    }
    Some(*header)
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn test_empty_graph_writes_valid_file() {
        let tmp = tempfile::tempdir().unwrap();
        let writer = CausalWriter::new(tmp.path());
        let records = vec![make_record("a", vec![], vec![]), make_record("b", vec![], vec![])];

        let report = writer.build(&records, &[], 1);

        assert_eq!(report.node_count, 2);
        assert_eq!(report.edge_count, 0);
        assert!(!report.skipped_unchanged);

        // Verify file exists and has correct size
        let csr_path = tmp.path().join("causal.csr");
        assert!(csr_path.exists());

        let data = std::fs::read(&csr_path).unwrap();
        let header: &CausalHeader = bytemuck::from_bytes(&data[..64]);
        assert_eq!(header.magic, CAUSAL_MAGIC);
        assert_eq!(header.version, CAUSAL_VERSION);
        assert_eq!(header.node_count, 2);
        assert_eq!(header.edge_count, 0);
        assert_eq!(header.generation, 1);
        assert_eq!(data.len(), engram_core::expected_file_size(header));
    }

    #[test]
    fn test_single_edge_forward_and_backward() {
        let tmp = tempfile::tempdir().unwrap();
        let writer = CausalWriter::new(tmp.path());
        let records = vec![
            make_record("a", vec![], vec!["b"]),
            make_record("b", vec![], vec![]),
        ];

        let report = writer.build(&records, &[], 1);

        assert_eq!(report.node_count, 2);
        assert_eq!(report.edge_count, 1);

        let data = std::fs::read(tmp.path().join("causal.csr")).unwrap();
        let header: &CausalHeader = bytemuck::from_bytes(&data[..64]);
        assert_eq!(data.len(), engram_core::expected_file_size(header));
    }

    /// Read a CausalNode from a byte slice at an arbitrary offset (may be unaligned).
    fn read_node(data: &[u8], offset: usize) -> CausalNode {
        let size = std::mem::size_of::<CausalNode>();
        bytemuck::pod_read_unaligned::<CausalNode>(&data[offset..offset + size])
    }

    #[test]
    fn test_lexicographic_node_ordering() {
        let tmp = tempfile::tempdir().unwrap();
        let writer = CausalWriter::new(tmp.path());
        // "c" comes after "a" and "b" lexicographically
        let records = vec![
            make_record("c", vec![], vec![]),
            make_record("a", vec![], vec![]),
            make_record("b", vec![], vec![]),
        ];

        let report = writer.build(&records, &[], 1);
        assert_eq!(report.node_count, 3);

        // Read string table to verify order: a, b, c
        let data = std::fs::read(tmp.path().join("causal.csr")).unwrap();
        let header: &CausalHeader = bytemuck::from_bytes(&data[..64]);
        let st_bytes = header.string_table_bytes as usize;
        let st = &data[64..64 + st_bytes];

        let padded_st = align_up(st_bytes, 4);
        let node_start = 64 + padded_st;
        let node_size = std::mem::size_of::<CausalNode>();

        let node0 = read_node(&data, node_start);
        let id0 = std::str::from_utf8(&st[node0.id_offset as usize..(node0.id_offset + node0.id_len) as usize]).unwrap();
        assert_eq!(id0, "a");

        let node1 = read_node(&data, node_start + node_size);
        let id1 = std::str::from_utf8(&st[node1.id_offset as usize..(node1.id_offset + node1.id_len) as usize]).unwrap();
        assert_eq!(id1, "b");

        let node2 = read_node(&data, node_start + 2 * node_size);
        let id2 = std::str::from_utf8(&st[node2.id_offset as usize..(node2.id_offset + node2.id_len) as usize]).unwrap();
        assert_eq!(id2, "c");
    }

    #[test]
    fn test_dangling_edges_excluded_from_csr() {
        let tmp = tempfile::tempdir().unwrap();
        let writer = CausalWriter::new(tmp.path());
        let records = vec![make_record("a", vec![], vec!["nonexistent"])];

        let report = writer.build(&records, &[], 1);

        assert_eq!(report.edge_count, 0);
        assert_eq!(report.dangling_edges_dropped, 1);
    }

    #[test]
    fn test_self_loops_excluded_from_csr() {
        let tmp = tempfile::tempdir().unwrap();
        let writer = CausalWriter::new(tmp.path());
        let records = vec![make_record("a", vec![], vec!["a"])];

        let report = writer.build(&records, &[], 1);

        assert_eq!(report.edge_count, 0);
    }

    #[test]
    fn test_duplicate_edges_deduplicated() {
        let tmp = tempfile::tempdir().unwrap();
        let writer = CausalWriter::new(tmp.path());
        // Both sides declare the same edge
        let records = vec![
            make_record("a", vec![], vec!["b"]),
            make_record("b", vec!["a"], vec![]),
        ];

        let report = writer.build(&records, &[], 1);

        assert_eq!(report.edge_count, 1);
        assert_eq!(report.duplicate_edges_removed, 1); // 2 declarations, 1 edge, 0 dropped
    }

    #[test]
    fn test_cycles_kept_in_csr() {
        let tmp = tempfile::tempdir().unwrap();
        let writer = CausalWriter::new(tmp.path());
        // a → b → a (cycle)
        let records = vec![
            make_record("a", vec![], vec!["b"]),
            make_record("b", vec![], vec!["a"]),
        ];

        let report = writer.build(&records, &[], 1);

        // Both edges should be present
        assert_eq!(report.edge_count, 2);
    }

    #[test]
    fn test_fingerprint_deterministic() {
        let tmp1 = tempfile::tempdir().unwrap();
        let tmp2 = tempfile::tempdir().unwrap();
        let records = vec![
            make_record("a", vec![], vec!["b"]),
            make_record("b", vec![], vec!["c"]),
            make_record("c", vec![], vec![]),
        ];

        let r1 = CausalWriter::new(tmp1.path()).build(&records, &[], 1);
        let r2 = CausalWriter::new(tmp2.path()).build(&records, &[], 1);

        assert_eq!(r1.graph_fingerprint, r2.graph_fingerprint);
    }

    #[test]
    fn test_fingerprint_changes_with_edges() {
        let tmp1 = tempfile::tempdir().unwrap();
        let tmp2 = tempfile::tempdir().unwrap();
        let records1 = vec![
            make_record("a", vec![], vec!["b"]),
            make_record("b", vec![], vec![]),
        ];
        let records2 = vec![
            make_record("a", vec![], vec![]),
            make_record("b", vec![], vec![]),
        ];

        let r1 = CausalWriter::new(tmp1.path()).build(&records1, &[], 1);
        let r2 = CausalWriter::new(tmp2.path()).build(&records2, &[], 1);

        assert_ne!(r1.graph_fingerprint, r2.graph_fingerprint);
    }

    #[test]
    fn test_incremental_gate_skips_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        let writer = CausalWriter::new(tmp.path());
        let records = vec![
            make_record("a", vec![], vec!["b"]),
            make_record("b", vec![], vec![]),
        ];

        let r1 = writer.build(&records, &[], 1);
        assert!(!r1.skipped_unchanged);

        // Second build with same data and same generation → skip
        let r2 = writer.build(&records, &[], 1);
        assert!(r2.skipped_unchanged);
        assert_eq!(r1.graph_fingerprint, r2.graph_fingerprint);
    }

    #[test]
    fn test_incremental_gate_rebuilds_on_generation_change() {
        let tmp = tempfile::tempdir().unwrap();
        let writer = CausalWriter::new(tmp.path());
        let records = vec![
            make_record("a", vec![], vec!["b"]),
            make_record("b", vec![], vec![]),
        ];

        let r1 = writer.build(&records, &[], 1);
        assert!(!r1.skipped_unchanged);

        // Same fingerprint but different generation → rebuild
        let r2 = writer.build(&records, &[], 2);
        assert!(!r2.skipped_unchanged);
    }

    #[test]
    fn test_node_metadata_populated() {
        let tmp = tempfile::tempdir().unwrap();
        let writer = CausalWriter::new(tmp.path());

        let mut rec = make_record("x", vec![], vec!["y"]);
        rec.importance = 0.75;
        rec.fact_type = FactType::Event;

        let records = vec![rec, make_record("y", vec![], vec![])];

        writer.build(&records, &[], 1);

        let data = std::fs::read(tmp.path().join("causal.csr")).unwrap();
        let header: &CausalHeader = bytemuck::from_bytes(&data[..64]);
        let padded_st = align_up(header.string_table_bytes as usize, 4);
        let node_start = 64 + padded_st;
        let node_size = std::mem::size_of::<CausalNode>();

        // "x" < "y" lexicographically, so node 0 = "x"
        let node0 = read_node(&data, node_start);
        assert_eq!(node0.fact_type, 2); // Event
        assert_eq!(f64::from_bits(node0.importance_bits), 0.75);
        assert_eq!(node0.out_degree, 1);
        assert_eq!(node0.in_degree, 0);
        assert_eq!(
            node0.source_path_hash,
            engram_core::temporal::fnv1a_64(b"x.md"),
        );

        let node1 = read_node(&data, node_start + node_size);
        assert_eq!(node1.fact_type, 0); // Durable (default)
        assert_eq!(node1.out_degree, 0);
        assert_eq!(node1.in_degree, 1);
        assert_eq!(
            node1.source_path_hash,
            engram_core::temporal::fnv1a_64(b"y.md"),
        );
    }

    #[test]
    fn test_build_report_fully_populated() {
        let tmp = tempfile::tempdir().unwrap();
        let writer = CausalWriter::new(tmp.path());
        let records = vec![
            make_record("a", vec![], vec!["b", "ghost"]),
            make_record("b", vec!["a"], vec![]),
            make_record("c", vec![], vec!["c"]),
        ];
        let warnings = vec![
            CausalValidationWarning::DanglingEdge {
                source_path: "a.md".to_string(),
                source_id: "a".to_string(),
                target_id: "ghost".to_string(),
            },
            CausalValidationWarning::SelfLoop {
                fact_id: "c".to_string(),
            },
        ];

        let report = writer.build(&records, &warnings, 5);

        assert_eq!(report.node_count, 3);
        assert_eq!(report.edge_count, 1); // only a→b survives
        assert_eq!(report.dangling_edges_dropped, 1);
        assert!(!report.skipped_unchanged);
        assert_eq!(report.warnings.len(), 2);
        assert_ne!(report.graph_fingerprint, [0u8; 16]);
        assert!(report.build_ms < 1000); // sanity
    }

    #[test]
    fn test_empty_records_produces_empty_file() {
        let tmp = tempfile::tempdir().unwrap();
        let writer = CausalWriter::new(tmp.path());

        let report = writer.build(&[], &[], 1);

        assert_eq!(report.node_count, 0);
        assert_eq!(report.edge_count, 0);

        let csr_path = tmp.path().join("causal.csr");
        assert!(csr_path.exists());

        let data = std::fs::read(&csr_path).unwrap();
        let header: &CausalHeader = bytemuck::from_bytes(&data[..64]);
        assert_eq!(data.len(), engram_core::expected_file_size(header));
    }

    #[test]
    fn test_generation_written_to_header() {
        let tmp = tempfile::tempdir().unwrap();
        let writer = CausalWriter::new(tmp.path());
        let records = vec![make_record("a", vec![], vec![])];

        writer.build(&records, &[], 42);

        let data = std::fs::read(tmp.path().join("causal.csr")).unwrap();
        let header: &CausalHeader = bytemuck::from_bytes(&data[..64]);
        assert_eq!(header.generation, 42);
    }
}
