//! Zero-copy reader for the causal graph CSR file.
//!
//! Loads `.brv/index/causal.csr` into memory, parses all sections, and
//! exposes graph traversal APIs for the query pipeline.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::path::Path;

use engram_core::{
    align_up, validate_causal_header, CausalEdge, CausalHeader, CausalNode,
};

/// Decay base for causal adjacency scoring. A node at hop distance `d`
/// receives a score of `CAUSAL_DECAY_BASE ^ d`.
pub const CAUSAL_DECAY_BASE: f64 = 0.7;

/// Direction for bounded BFS traversal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraversalDirection {
    /// Follow forward edges (causes → effects).
    Forward,
    /// Follow backward edges (effects → causes).
    Backward,
}

/// Error type for causal reader operations.
#[derive(Debug, thiserror::Error)]
pub enum CausalReadError {
    #[error("causal.csr not found")]
    NotFound,

    #[error("causal.csr is too small ({0} bytes, need at least 64)")]
    TooSmall(usize),

    #[error("invalid causal.csr header: {0}")]
    InvalidHeader(String),

    #[error("causal.csr file size mismatch: expected {expected}, got {actual}")]
    SizeMismatch { expected: usize, actual: usize },

    #[error("causal.csr generation mismatch: file has {file_gen}, index has {index_gen}")]
    GenerationMismatch { file_gen: u64, index_gen: u64 },

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Zero-copy reader for the causal graph.
///
/// Constructed once per query session. All data lives in a single `Vec<u8>`
/// buffer; the parsed sections are byte-range references into it.
pub struct CausalReader {
    /// Raw file bytes.
    buf: Vec<u8>,
    /// Parsed header (copied out, always aligned).
    header: CausalHeader,
    /// Byte offset where the string table starts.
    string_table_start: usize,
    /// Byte offset where the node table starts.
    node_table_start: usize,
    /// Byte offset where forward CSR offsets start.
    fwd_offsets_start: usize,
    /// Byte offset where forward CSR targets start.
    fwd_targets_start: usize,
    /// Byte offset where backward CSR offsets start.
    bwd_offsets_start: usize,
    /// Byte offset where backward CSR targets start.
    bwd_targets_start: usize,
}

impl fmt::Debug for CausalReader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CausalReader")
            .field("header", &self.header)
            .field("buf_len", &self.buf.len())
            .finish()
    }
}

impl CausalReader {
    /// Load `causal.csr` from the given index directory.
    ///
    /// Validates magic, version, file size, and generation.
    /// Returns `CausalReadError::NotFound` if the file does not exist.
    /// Returns `CausalReadError::GenerationMismatch` if the file's generation
    /// does not match `expected_generation`.
    pub fn load(index_dir: &Path, expected_generation: u64) -> Result<Self, CausalReadError> {
        let path = index_dir.join("causal.csr");
        if !path.exists() {
            return Err(CausalReadError::NotFound);
        }

        let buf = std::fs::read(&path)?;

        if buf.len() < 64 {
            return Err(CausalReadError::TooSmall(buf.len()));
        }

        let header: CausalHeader = bytemuck::pod_read_unaligned(&buf[..64]);

        validate_causal_header(&header)
            .map_err(|e| CausalReadError::InvalidHeader(e.to_string()))?;

        let expected_size = engram_core::expected_file_size(&header);
        if buf.len() != expected_size {
            return Err(CausalReadError::SizeMismatch {
                expected: expected_size,
                actual: buf.len(),
            });
        }

        if header.generation != expected_generation {
            return Err(CausalReadError::GenerationMismatch {
                file_gen: header.generation,
                index_gen: expected_generation,
            });
        }

        let n = header.node_count as usize;
        let e = header.edge_count as usize;
        let st_padded = align_up(header.string_table_bytes as usize, 4);
        let node_size = std::mem::size_of::<CausalNode>();
        let edge_size = std::mem::size_of::<CausalEdge>();

        let string_table_start = 64;
        let node_table_start = string_table_start + st_padded;
        let fwd_offsets_start = node_table_start + n * node_size;
        let fwd_targets_start = fwd_offsets_start + (n + 1) * 4;
        let bwd_offsets_start = fwd_targets_start + e * edge_size;
        let bwd_targets_start = bwd_offsets_start + (n + 1) * 4;

        Ok(CausalReader {
            buf,
            header,
            string_table_start,
            node_table_start,
            fwd_offsets_start,
            fwd_targets_start,
            bwd_offsets_start,
            bwd_targets_start,
        })
    }

    /// Create an empty reader that satisfies all API calls with zero results.
    /// Used when `causal.csr` does not exist or is stale.
    pub fn empty() -> Self {
        let header = CausalHeader {
            magic: engram_core::CAUSAL_MAGIC,
            version: engram_core::CAUSAL_VERSION,
            node_count: 0,
            edge_count: 0,
            string_table_bytes: 0,
            generation: 0,
            graph_fingerprint: [0; 16],
            _pad: [0; 16],
        };

        // Minimal valid buffer: 64-byte header + 4-byte fwd sentinel + 4-byte bwd sentinel
        let mut buf = vec![0u8; 72];
        buf[..64].copy_from_slice(bytemuck::bytes_of(&header));

        CausalReader {
            buf,
            header,
            string_table_start: 64,
            node_table_start: 64,
            fwd_offsets_start: 64,
            fwd_targets_start: 68,
            bwd_offsets_start: 68,
            bwd_targets_start: 72,
        }
    }

    /// Number of nodes in the graph.
    pub fn node_count(&self) -> u32 {
        self.header.node_count
    }

    /// Number of edges in the graph.
    pub fn edge_count(&self) -> u32 {
        self.header.edge_count
    }

    /// Generation of the loaded graph.
    pub fn generation(&self) -> u64 {
        self.header.generation
    }

    // ─── Node access ────────────────────────────────────────────────────

    /// Read a `CausalNode` by index. Returns `None` if out of range.
    fn node(&self, index: u32) -> Option<CausalNode> {
        if index >= self.header.node_count {
            return None;
        }
        let node_size = std::mem::size_of::<CausalNode>();
        let offset = self.node_table_start + (index as usize) * node_size;
        Some(bytemuck::pod_read_unaligned(&self.buf[offset..offset + node_size]))
    }

    /// Read the fact_id string for a given node index. Returns `None` if out of range.
    pub fn node_fact_id(&self, index: u32) -> Option<&str> {
        let node = self.node(index)?;
        let start = self.string_table_start + node.id_offset as usize;
        let end = start + node.id_len as usize;
        if end > self.string_table_start + self.header.string_table_bytes as usize {
            return None;
        }
        std::str::from_utf8(&self.buf[start..end]).ok()
    }

    // ─── String lookup ──────────────────────────────────────────────────

    /// Given a fact ID string, return its node index using binary search
    /// over the lexicographically sorted node table. O(log N).
    pub fn fact_id_to_node(&self, fact_id: &str) -> Option<u32> {
        let n = self.header.node_count;
        if n == 0 {
            return None;
        }

        let mut lo: u32 = 0;
        let mut hi: u32 = n;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let mid_id = self.node_fact_id(mid)?;
            match mid_id.cmp(fact_id) {
                std::cmp::Ordering::Equal => return Some(mid),
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
            }
        }
        None
    }

    // ─── CSR access ─────────────────────────────────────────────────────

    /// Read a u32 from the buffer at the given byte offset (little-endian).
    fn read_u32(&self, offset: usize) -> u32 {
        let bytes: [u8; 4] = self.buf[offset..offset + 4].try_into().unwrap_or([0; 4]);
        u32::from_le_bytes(bytes)
    }

    /// Return forward neighbors (outgoing edges) for a node.
    /// Returns an empty slice for out-of-range indices or nodes with no edges.
    pub fn forward_neighbors(&self, node_index: u32) -> Vec<CausalEdge> {
        self.neighbors_from(node_index, self.fwd_offsets_start, self.fwd_targets_start)
    }

    /// Return backward neighbors (incoming edges) for a node.
    /// Returns an empty slice for out-of-range indices or nodes with no edges.
    pub fn backward_neighbors(&self, node_index: u32) -> Vec<CausalEdge> {
        self.neighbors_from(node_index, self.bwd_offsets_start, self.bwd_targets_start)
    }

    /// Generic neighbor lookup from a CSR offset/target array pair.
    fn neighbors_from(
        &self,
        node_index: u32,
        offsets_start: usize,
        targets_start: usize,
    ) -> Vec<CausalEdge> {
        if node_index >= self.header.node_count {
            return Vec::new();
        }
        let start = self.read_u32(offsets_start + (node_index as usize) * 4) as usize;
        let end = self.read_u32(offsets_start + (node_index as usize + 1) * 4) as usize;
        if start >= end {
            return Vec::new();
        }

        let edge_size = std::mem::size_of::<CausalEdge>();
        let mut edges = Vec::with_capacity(end - start);
        for i in start..end {
            let off = targets_start + i * edge_size;
            edges.push(bytemuck::pod_read_unaligned(&self.buf[off..off + edge_size]));
        }
        edges
    }

    // ─── Traversal ──────────────────────────────────────────────────────

    /// BFS shortest path from `source` to `target` using forward edges only.
    /// Returns `None` if no path exists within `max_hops`.
    /// The returned path includes both source and target.
    pub fn shortest_path(&self, source: u32, target: u32, max_hops: u8) -> Option<Vec<u32>> {
        if source >= self.header.node_count || target >= self.header.node_count {
            return None;
        }
        if source == target {
            return Some(vec![source]);
        }

        // BFS with parent tracking
        let mut visited: HashSet<u32> = HashSet::new();
        let mut parent: HashMap<u32, u32> = HashMap::new();
        let mut queue: VecDeque<(u32, u8)> = VecDeque::new();

        visited.insert(source);
        queue.push_back((source, 0));

        while let Some((node, depth)) = queue.pop_front() {
            if depth >= max_hops {
                continue;
            }

            for edge in self.forward_neighbors(node) {
                let neighbor = edge.target_node;
                if visited.contains(&neighbor) {
                    continue;
                }
                visited.insert(neighbor);
                parent.insert(neighbor, node);

                if neighbor == target {
                    // Reconstruct path
                    let mut path = vec![target];
                    let mut cur = target;
                    while cur != source {
                        cur = parent[&cur];
                        path.push(cur);
                    }
                    path.reverse();
                    return Some(path);
                }

                queue.push_back((neighbor, depth + 1));
            }
        }

        None
    }

    /// Bounded BFS: return all nodes reachable from `source` within `max_hops`,
    /// paired with their hop distance. Direction selects forward or backward edges.
    pub fn reachable_within(
        &self,
        source: u32,
        max_hops: u8,
        direction: TraversalDirection,
    ) -> Vec<(u32, u8)> {
        if source >= self.header.node_count {
            return Vec::new();
        }

        let mut visited: HashSet<u32> = HashSet::new();
        let mut result: Vec<(u32, u8)> = Vec::new();
        let mut queue: VecDeque<(u32, u8)> = VecDeque::new();

        visited.insert(source);
        queue.push_back((source, 0));

        while let Some((node, depth)) = queue.pop_front() {
            if depth > 0 {
                result.push((node, depth));
            }
            if depth >= max_hops {
                continue;
            }

            let neighbors = match direction {
                TraversalDirection::Forward => self.forward_neighbors(node),
                TraversalDirection::Backward => self.backward_neighbors(node),
            };

            for edge in neighbors {
                let neighbor = edge.target_node;
                if visited.contains(&neighbor) {
                    continue;
                }
                visited.insert(neighbor);
                queue.push_back((neighbor, depth + 1));
            }
        }

        result
    }

    // ─── Scoring ────────────────────────────────────────────────────────

    /// Compute causal adjacency score between a source and candidate.
    ///
    /// Returns `CAUSAL_DECAY_BASE ^ hop_distance` if a path exists within
    /// `max_hops`. Returns `0.0` if no path exists. Returns `1.0` (neutral)
    /// if the candidate fact_id is not in the causal graph at all.
    pub fn causal_adjacency(
        &self,
        source_fact_id: &str,
        candidate_fact_id: &str,
        max_hops: u8,
    ) -> f64 {
        let source_idx = match self.fact_id_to_node(source_fact_id) {
            Some(idx) => idx,
            None => return 0.0, // source not in graph → adjacency undefined
        };
        let candidate_idx = match self.fact_id_to_node(candidate_fact_id) {
            Some(idx) => idx,
            None => return 1.0, // candidate not in graph → neutral
        };

        if source_idx == candidate_idx {
            return 1.0;
        }

        // BFS to find shortest hop distance
        let mut visited: HashSet<u32> = HashSet::new();
        let mut queue: VecDeque<(u32, u8)> = VecDeque::new();
        visited.insert(source_idx);
        queue.push_back((source_idx, 0));

        while let Some((node, depth)) = queue.pop_front() {
            if depth >= max_hops {
                continue;
            }
            for edge in self.forward_neighbors(node) {
                let neighbor = edge.target_node;
                if neighbor == candidate_idx {
                    return CAUSAL_DECAY_BASE.powi((depth + 1) as i32);
                }
                if visited.contains(&neighbor) {
                    continue;
                }
                visited.insert(neighbor);
                queue.push_back((neighbor, depth + 1));
            }
        }

        0.0 // no path within max_hops
    }

    // ─── Temporal join key ──────────────────────────────────────────────

    /// Return the `source_path_hash` for a node. This is the join key for
    /// temporal hit enrichment.
    pub fn node_source_path_hash(&self, node_index: u32) -> Option<u64> {
        self.node(node_index).map(|n| n.source_path_hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engram_core::{
        align_up, quantize_weight, CausalEdge, CausalHeader, CausalNode,
        CAUSAL_MAGIC, CAUSAL_VERSION,
    };

    /// Build a test graph in memory and return a CausalReader.
    ///
    /// Graph structure (7 nodes, 8 edges including a cycle):
    ///
    ///   a → b → d → e
    ///   a → c → d
    ///   e → b        (cycle: b → d → e → b)
    ///   a → f
    ///   g             (disconnected)
    ///
    /// Sorted IDs: a(0), b(1), c(2), d(3), e(4), f(5), g(6)
    ///
    /// Edges (source → target):
    ///   a→b  a→c  a→f  b→d  c→d  d→e  e→b  (+ e→b creates cycle b→d→e→b)
    /// Wait — let me count: a→b, a→c, a→f, b→d, c→d, d→e, e→b = 7 edges.
    /// Adding one more: f→e to get 8 edges.
    ///
    /// Final edges: a→b, a→c, a→f, b→d, c→d, d→e, e→b, f→e
    fn build_test_graph(generation: u64) -> Vec<u8> {
        // Sorted fact IDs and their indices
        let ids = ["a", "b", "c", "d", "e", "f", "g"];
        let n = ids.len();

        // Forward edges: (source_idx, target_idx)
        let edges: Vec<(u32, u32)> = vec![
            (0, 1), // a→b
            (0, 2), // a→c
            (0, 5), // a→f
            (1, 3), // b→d
            (2, 3), // c→d
            (3, 4), // d→e
            (4, 1), // e→b (cycle)
            (5, 4), // f→e
        ];
        let e = edges.len();

        // Build string table
        let mut string_table = Vec::new();
        let mut id_offsets: Vec<(u32, u32)> = Vec::new();
        for id in &ids {
            let offset = string_table.len() as u32;
            let bytes = id.as_bytes();
            string_table.extend_from_slice(bytes);
            id_offsets.push((offset, bytes.len() as u32));
        }
        let string_table_bytes = string_table.len() as u32;
        let st_padded = align_up(string_table.len(), 4);

        // Build node table
        // Compute out_degree and in_degree
        let mut out_deg = vec![0u8; n];
        let mut in_deg = vec![0u8; n];
        for &(s, t) in &edges {
            out_deg[s as usize] += 1;
            in_deg[t as usize] += 1;
        }

        let mut nodes = Vec::new();
        for (i, &(off, len)) in id_offsets.iter().enumerate() {
            let path = format!("{}.md", ids[i]);
            let hash = engram_core::temporal::fnv1a_64(path.as_bytes());
            nodes.push(CausalNode {
                id_offset: off,
                id_len: len,
                source_path_hash: hash,
                importance_bits: 1.0f64.to_bits(),
                fact_type: 0,
                out_degree: out_deg[i],
                in_degree: in_deg[i],
                _pad: [0; 5],
            });
        }

        // Build forward CSR
        let mut fwd_adj: Vec<Vec<CausalEdge>> = vec![Vec::new(); n];
        for &(s, t) in &edges {
            fwd_adj[s as usize].push(CausalEdge {
                target_node: t,
                weight: quantize_weight(1.0),
                _pad: [0; 2],
            });
        }
        for adj in &mut fwd_adj {
            adj.sort_by_key(|e| e.target_node);
        }

        let mut fwd_offsets = Vec::new();
        let mut running = 0u32;
        for adj in &fwd_adj {
            fwd_offsets.push(running);
            running += adj.len() as u32;
        }
        fwd_offsets.push(running);
        let fwd_targets: Vec<CausalEdge> = fwd_adj.into_iter().flatten().collect();

        // Build backward CSR (transpose)
        let mut bwd_adj: Vec<Vec<CausalEdge>> = vec![Vec::new(); n];
        for &(s, t) in &edges {
            bwd_adj[t as usize].push(CausalEdge {
                target_node: s,
                weight: quantize_weight(1.0),
                _pad: [0; 2],
            });
        }
        for adj in &mut bwd_adj {
            adj.sort_by_key(|e| e.target_node);
        }

        let mut bwd_offsets = Vec::new();
        let mut running = 0u32;
        for adj in &bwd_adj {
            bwd_offsets.push(running);
            running += adj.len() as u32;
        }
        bwd_offsets.push(running);
        let bwd_targets: Vec<CausalEdge> = bwd_adj.into_iter().flatten().collect();

        // Assemble header
        let header = CausalHeader {
            magic: CAUSAL_MAGIC,
            version: CAUSAL_VERSION,
            node_count: n as u32,
            edge_count: e as u32,
            string_table_bytes,
            generation,
            graph_fingerprint: [0; 16],
            _pad: [0; 16],
        };

        // Serialize
        let total = engram_core::expected_file_size(&header);
        let mut buf = Vec::with_capacity(total);

        buf.extend_from_slice(bytemuck::bytes_of(&header));
        buf.extend_from_slice(&string_table);
        buf.resize(64 + st_padded, 0);
        for node in &nodes {
            buf.extend_from_slice(bytemuck::bytes_of(node));
        }
        for &off in &fwd_offsets {
            buf.extend_from_slice(&off.to_le_bytes());
        }
        for edge in &fwd_targets {
            buf.extend_from_slice(bytemuck::bytes_of(edge));
        }
        for &off in &bwd_offsets {
            buf.extend_from_slice(&off.to_le_bytes());
        }
        for edge in &bwd_targets {
            buf.extend_from_slice(bytemuck::bytes_of(edge));
        }
        assert_eq!(buf.len(), total);
        buf
    }

    fn write_and_load(dir: &Path, generation: u64) -> CausalReader {
        let buf = build_test_graph(generation);
        std::fs::write(dir.join("causal.csr"), &buf).unwrap();
        CausalReader::load(dir, generation).unwrap()
    }

    // ─── Basic load tests ───────────────────────────────────────────────

    #[test]
    fn test_load_valid_graph() {
        let tmp = tempfile::tempdir().unwrap();
        let reader = write_and_load(tmp.path(), 1);
        assert_eq!(reader.node_count(), 7);
        assert_eq!(reader.edge_count(), 8);
        assert_eq!(reader.generation(), 1);
    }

    #[test]
    fn test_load_missing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let err = CausalReader::load(tmp.path(), 1).unwrap_err();
        assert!(matches!(err, CausalReadError::NotFound));
    }

    #[test]
    fn test_load_generation_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let buf = build_test_graph(5);
        std::fs::write(tmp.path().join("causal.csr"), &buf).unwrap();
        let err = CausalReader::load(tmp.path(), 10).unwrap_err();
        assert!(matches!(err, CausalReadError::GenerationMismatch { .. }));
    }

    #[test]
    fn test_load_corrupt_file() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("causal.csr"), b"too short").unwrap();
        let err = CausalReader::load(tmp.path(), 1).unwrap_err();
        assert!(matches!(err, CausalReadError::TooSmall(_)));
    }

    #[test]
    fn test_load_bad_magic() {
        let tmp = tempfile::tempdir().unwrap();
        let mut buf = build_test_graph(1);
        buf[0..8].copy_from_slice(b"BADMAGIC");
        std::fs::write(tmp.path().join("causal.csr"), &buf).unwrap();
        let err = CausalReader::load(tmp.path(), 1).unwrap_err();
        assert!(matches!(err, CausalReadError::InvalidHeader(_)));
    }

    // ─── Empty reader tests ─────────────────────────────────────────────

    #[test]
    fn test_empty_reader() {
        let reader = CausalReader::empty();
        assert_eq!(reader.node_count(), 0);
        assert_eq!(reader.edge_count(), 0);
        assert_eq!(reader.fact_id_to_node("anything"), None);
        assert!(reader.forward_neighbors(0).is_empty());
        assert!(reader.backward_neighbors(0).is_empty());
        assert_eq!(reader.shortest_path(0, 1, 10), None);
        assert!(reader.reachable_within(0, 10, TraversalDirection::Forward).is_empty());
        assert_eq!(reader.causal_adjacency("a", "b", 5), 0.0); // source absent → 0.0
        assert_eq!(reader.node_source_path_hash(0), None);
    }

    // ─── fact_id_to_node tests ──────────────────────────────────────────

    #[test]
    fn test_fact_id_lookup() {
        let tmp = tempfile::tempdir().unwrap();
        let reader = write_and_load(tmp.path(), 1);

        assert_eq!(reader.fact_id_to_node("a"), Some(0));
        assert_eq!(reader.fact_id_to_node("b"), Some(1));
        assert_eq!(reader.fact_id_to_node("c"), Some(2));
        assert_eq!(reader.fact_id_to_node("d"), Some(3));
        assert_eq!(reader.fact_id_to_node("e"), Some(4));
        assert_eq!(reader.fact_id_to_node("f"), Some(5));
        assert_eq!(reader.fact_id_to_node("g"), Some(6));
        assert_eq!(reader.fact_id_to_node("nonexistent"), None);
    }

    // ─── Neighbor tests ─────────────────────────────────────────────────

    #[test]
    fn test_forward_neighbors() {
        let tmp = tempfile::tempdir().unwrap();
        let reader = write_and_load(tmp.path(), 1);

        // a(0) → b(1), c(2), f(5)
        let fwd_a: Vec<u32> = reader.forward_neighbors(0).iter().map(|e| e.target_node).collect();
        assert_eq!(fwd_a, vec![1, 2, 5]);

        // b(1) → d(3)
        let fwd_b: Vec<u32> = reader.forward_neighbors(1).iter().map(|e| e.target_node).collect();
        assert_eq!(fwd_b, vec![3]);

        // g(6) → nothing
        assert!(reader.forward_neighbors(6).is_empty());

        // out of range
        assert!(reader.forward_neighbors(100).is_empty());
    }

    #[test]
    fn test_backward_neighbors() {
        let tmp = tempfile::tempdir().unwrap();
        let reader = write_and_load(tmp.path(), 1);

        // b(1) ← a(0), e(4)
        let bwd_b: Vec<u32> = reader.backward_neighbors(1).iter().map(|e| e.target_node).collect();
        assert_eq!(bwd_b, vec![0, 4]);

        // d(3) ← b(1), c(2)
        let bwd_d: Vec<u32> = reader.backward_neighbors(3).iter().map(|e| e.target_node).collect();
        assert_eq!(bwd_d, vec![1, 2]);

        // a(0) ← nothing
        assert!(reader.backward_neighbors(0).is_empty());

        // g(6) ← nothing (disconnected)
        assert!(reader.backward_neighbors(6).is_empty());
    }

    // ─── Shortest path tests ────────────────────────────────────────────

    #[test]
    fn test_shortest_path_direct() {
        let tmp = tempfile::tempdir().unwrap();
        let reader = write_and_load(tmp.path(), 1);

        // a→b is 1 hop
        let path = reader.shortest_path(0, 1, 5).unwrap();
        assert_eq!(path, vec![0, 1]);
    }

    #[test]
    fn test_shortest_path_multi_hop() {
        let tmp = tempfile::tempdir().unwrap();
        let reader = write_and_load(tmp.path(), 1);

        // a→b→d is 2 hops (shorter than a→c→d which is also 2)
        let path = reader.shortest_path(0, 3, 5).unwrap();
        assert_eq!(path.len(), 3);
        assert_eq!(path[0], 0); // a
        assert_eq!(path[2], 3); // d
        // Middle could be b(1) or c(2) — both are 2-hop paths
    }

    #[test]
    fn test_shortest_path_through_cycle() {
        let tmp = tempfile::tempdir().unwrap();
        let reader = write_and_load(tmp.path(), 1);

        // a→b→d→e is 3 hops
        let path = reader.shortest_path(0, 4, 5).unwrap();
        assert!(path.len() <= 4); // at most 4 nodes in the path
        assert_eq!(*path.first().unwrap(), 0);
        assert_eq!(*path.last().unwrap(), 4);
    }

    #[test]
    fn test_shortest_path_same_node() {
        let tmp = tempfile::tempdir().unwrap();
        let reader = write_and_load(tmp.path(), 1);

        let path = reader.shortest_path(0, 0, 5).unwrap();
        assert_eq!(path, vec![0]);
    }

    #[test]
    fn test_shortest_path_no_path() {
        let tmp = tempfile::tempdir().unwrap();
        let reader = write_and_load(tmp.path(), 1);

        // g(6) is disconnected — no path from a to g
        assert_eq!(reader.shortest_path(0, 6, 10), None);
    }

    #[test]
    fn test_shortest_path_depth_limit_enforced() {
        let tmp = tempfile::tempdir().unwrap();
        let reader = write_and_load(tmp.path(), 1);

        // Shortest path a→e is a→f→e (2 hops). With max_hops=1, not found.
        assert_eq!(reader.shortest_path(0, 4, 1), None);

        // With max_hops=2, should be found via a→f→e.
        let path = reader.shortest_path(0, 4, 2).unwrap();
        assert_eq!(path, vec![0, 5, 4]); // a→f→e
    }

    #[test]
    fn test_shortest_path_cycle_does_not_loop() {
        let tmp = tempfile::tempdir().unwrap();
        let reader = write_and_load(tmp.path(), 1);

        // The cycle b→d→e→b exists. Searching from e to g should not loop.
        // g is disconnected, so result should be None, not a hang.
        assert_eq!(reader.shortest_path(4, 6, 100), None);
    }

    // ─── Reachable within tests ─────────────────────────────────────────

    #[test]
    fn test_reachable_forward() {
        let tmp = tempfile::tempdir().unwrap();
        let reader = write_and_load(tmp.path(), 1);

        // From a(0) forward, max_hops=1: b(1), c(2), f(5)
        let reach = reader.reachable_within(0, 1, TraversalDirection::Forward);
        let nodes: HashSet<u32> = reach.iter().map(|&(n, _)| n).collect();
        assert_eq!(nodes, HashSet::from([1, 2, 5]));
        for &(_, d) in &reach {
            assert_eq!(d, 1);
        }
    }

    #[test]
    fn test_reachable_forward_2_hops() {
        let tmp = tempfile::tempdir().unwrap();
        let reader = write_and_load(tmp.path(), 1);

        // From a(0) forward, max_hops=2: b(1@1), c(2@1), f(5@1), d(3@2), e(4@2)
        let reach = reader.reachable_within(0, 2, TraversalDirection::Forward);
        let node_map: HashMap<u32, u8> = reach.into_iter().collect();
        assert_eq!(node_map.get(&1), Some(&1)); // b
        assert_eq!(node_map.get(&2), Some(&1)); // c
        assert_eq!(node_map.get(&5), Some(&1)); // f
        assert_eq!(node_map.get(&3), Some(&2)); // d
        assert_eq!(node_map.get(&4), Some(&2)); // e (via f→e)
    }

    #[test]
    fn test_reachable_backward() {
        let tmp = tempfile::tempdir().unwrap();
        let reader = write_and_load(tmp.path(), 1);

        // From d(3) backward, max_hops=1: b(1), c(2)
        let reach = reader.reachable_within(3, 1, TraversalDirection::Backward);
        let nodes: HashSet<u32> = reach.iter().map(|&(n, _)| n).collect();
        assert_eq!(nodes, HashSet::from([1, 2]));
    }

    #[test]
    fn test_reachable_backward_multi_hop() {
        let tmp = tempfile::tempdir().unwrap();
        let reader = write_and_load(tmp.path(), 1);

        // From d(3) backward, max_hops=2: b(1@1), c(2@1), a(0@2), e(4@2)
        // b←a and b←e, c←a
        let reach = reader.reachable_within(3, 2, TraversalDirection::Backward);
        let node_map: HashMap<u32, u8> = reach.into_iter().collect();
        assert_eq!(node_map.get(&1), Some(&1)); // b
        assert_eq!(node_map.get(&2), Some(&1)); // c
        assert_eq!(node_map.get(&0), Some(&2)); // a
        assert_eq!(node_map.get(&4), Some(&2)); // e
    }

    #[test]
    fn test_reachable_disconnected_node() {
        let tmp = tempfile::tempdir().unwrap();
        let reader = write_and_load(tmp.path(), 1);

        // g(6) is disconnected
        let reach = reader.reachable_within(6, 10, TraversalDirection::Forward);
        assert!(reach.is_empty());
    }

    #[test]
    fn test_reachable_cycle_does_not_loop() {
        let tmp = tempfile::tempdir().unwrap();
        let reader = write_and_load(tmp.path(), 1);

        // From b(1) forward with high depth — cycle exists but visited set prevents loop
        let reach = reader.reachable_within(1, 100, TraversalDirection::Forward);
        // Should find d(3), e(4), and back to b(1) is blocked by visited set
        let nodes: HashSet<u32> = reach.iter().map(|&(n, _)| n).collect();
        assert!(nodes.contains(&3)); // d
        assert!(nodes.contains(&4)); // e
        assert!(!nodes.contains(&1)); // b itself is source, not in result
    }

    // ─── Causal adjacency tests ─────────────────────────────────────────

    #[test]
    fn test_causal_adjacency_hop_0_same_node() {
        let tmp = tempfile::tempdir().unwrap();
        let reader = write_and_load(tmp.path(), 1);
        assert_eq!(reader.causal_adjacency("a", "a", 5), 1.0);
    }

    #[test]
    fn test_causal_adjacency_hop_1() {
        let tmp = tempfile::tempdir().unwrap();
        let reader = write_and_load(tmp.path(), 1);

        // a→b is 1 hop → 0.7^1 = 0.7
        let score = reader.causal_adjacency("a", "b", 5);
        assert!((score - 0.7).abs() < 1e-10);
    }

    #[test]
    fn test_causal_adjacency_hop_2() {
        let tmp = tempfile::tempdir().unwrap();
        let reader = write_and_load(tmp.path(), 1);

        // a→b→d is 2 hops → 0.7^2 = 0.49
        let score = reader.causal_adjacency("a", "d", 5);
        assert!((score - 0.49).abs() < 1e-10);
    }

    #[test]
    fn test_causal_adjacency_hop_2_via_f() {
        let tmp = tempfile::tempdir().unwrap();
        let reader = write_and_load(tmp.path(), 1);

        // Shortest path a→e is a→f→e (2 hops) → 0.7^2 = 0.49
        let score = reader.causal_adjacency("a", "e", 5);
        assert!((score - 0.49).abs() < 1e-10);
    }

    #[test]
    fn test_causal_adjacency_no_path() {
        let tmp = tempfile::tempdir().unwrap();
        let reader = write_and_load(tmp.path(), 1);

        // a to g — no path
        assert_eq!(reader.causal_adjacency("a", "g", 10), 0.0);
    }

    #[test]
    fn test_causal_adjacency_source_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let reader = write_and_load(tmp.path(), 1);

        // Source not in graph → adjacency undefined → 0.0
        assert_eq!(reader.causal_adjacency("xyz", "a", 5), 0.0);
    }

    #[test]
    fn test_causal_adjacency_candidate_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let reader = write_and_load(tmp.path(), 1);

        // Candidate not in graph → neutral → 1.0
        assert_eq!(reader.causal_adjacency("a", "xyz", 5), 1.0);
    }

    #[test]
    fn test_causal_adjacency_both_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let reader = write_and_load(tmp.path(), 1);

        // Both absent — source checked first → 0.0
        assert_eq!(reader.causal_adjacency("xyz", "zzz", 5), 0.0);
    }

    #[test]
    fn test_causal_adjacency_depth_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let reader = write_and_load(tmp.path(), 1);

        // Shortest a→e is a→f→e (2 hops). With max_hops=1, should return 0.0
        assert_eq!(reader.causal_adjacency("a", "e", 1), 0.0);
        // With max_hops=2, should return 0.7^2 = 0.49
        let score = reader.causal_adjacency("a", "e", 2);
        assert!((score - 0.49).abs() < 1e-10);
    }

    // ─── Node source_path_hash test ─────────────────────────────────────

    #[test]
    fn test_node_source_path_hash() {
        let tmp = tempfile::tempdir().unwrap();
        let reader = write_and_load(tmp.path(), 1);

        let hash_a = reader.node_source_path_hash(0).unwrap();
        assert_eq!(hash_a, engram_core::temporal::fnv1a_64(b"a.md"));

        let hash_g = reader.node_source_path_hash(6).unwrap();
        assert_eq!(hash_g, engram_core::temporal::fnv1a_64(b"g.md"));

        // Out of range
        assert_eq!(reader.node_source_path_hash(100), None);
    }
}
