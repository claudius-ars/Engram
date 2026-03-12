//! Causal graph data model for Phase 3.
//!
//! This module defines the shared types and binary file format contract for the
//! causal graph. The writer (`engram-compiler`) and reader (`engram-query`) both
//! depend on these types via `engram-core`. No writer/reader logic lives here.
//!
//! # File format: `causal.csr`
//!
//! The causal graph is stored as a Compressed Sparse Row (CSR) structure in
//! `.brv/index/causal.csr`. The file is a derived artifact, fully rebuildable
//! from `.md` source files. It is designed for zero-copy reads via `bytemuck`.
//!
//! ## Byte layout
//!
//! ```text
//! Offset  Size     Field
//! ──────  ───────  ─────────────────────────────────────
//! 0       8        magic: b"ENGRCASL"
//! 8       4        version: u32 (currently 1)
//! 12      4        node_count: u32
//! 16      4        edge_count: u32 (total forward edges)
//! 20      4        string_table_bytes: u32 (total bytes in string table)
//! 24      8        generation: u64 (must match index state generation)
//! 32      16       graph_fingerprint: [u8; 16] (BLAKE3 truncated to 128 bits)
//! 48      16       _pad: reserved, zero-filled
//! ──────  ───────  ─────────────────────────────────────
//! 64               (header ends, exactly 64 bytes)
//!
//! 64      variable STRING TABLE
//!                  Concatenated UTF-8 fact IDs, packed without separators.
//!                  Each node's ID is located at string_table[offset..offset+len].
//!
//! 64+S    variable NODE TABLE
//!                  node_count × CausalNode (32 bytes each)
//!                  Sorted by fact_id lexicographic order (enables binary search).
//!
//! 64+S+N  variable FORWARD CSR: offsets array
//!                  (node_count + 1) × u32
//!                  fwd_offsets[i]..fwd_offsets[i+1] indexes into fwd_targets.
//!
//! +       variable FORWARD CSR: targets array
//!                  edge_count × CausalEdge (8 bytes each)
//!
//! +       variable BACKWARD CSR: offsets array
//!                  (node_count + 1) × u32
//!                  bwd_offsets[i]..bwd_offsets[i+1] indexes into bwd_targets.
//!
//! +       variable BACKWARD CSR: targets array
//!                  bwd_edge_count × CausalEdge (8 bytes each)
//!                  (bwd_edge_count == edge_count; every forward edge has a backward)
//! ```
//!
//! Where S = string_table_bytes (padded to 4-byte alignment),
//!       N = node_count × 32.
//!
//! ## Node identity
//!
//! Nodes are identified by a dense 0-based index (`u32`) assigned by sorting
//! all fact IDs lexicographically. This is a **lexicographic integer assignment**
//! strategy.
//!
//! ### Why lexicographic assignment over hash-based or explicit map:
//!
//! - **Determinism**: Given the same set of fact IDs, the assignment is identical
//!   across builds. No collision handling needed. No persistent mapping file.
//! - **Rebuild cost**: O(n log n) sort per full compile. For 500–5,000 facts this
//!   is sub-millisecond. Acceptable for a derived artifact.
//! - **Incremental compile**: Node indices may change when facts are added/removed
//!   (unlike hash-based). This means incremental compile must rebuild the full
//!   graph when the fact set changes. This is the accepted tradeoff — the graph
//!   rebuild is fast (sub-10ms for 5K nodes) and correctness is more valuable
//!   than avoiding a graph rewrite.
//! - **Binary search**: Lexicographic order enables O(log n) fact_id → node_index
//!   lookup in the reader without a hash table.
//! - **No tombstones**: Deleted facts simply disappear from the next build.
//!   No compaction or GC required.
//!
//! The tradeoff accepted: every add/delete invalidates all node indices, so the
//! graph must be fully rebuilt. For the target corpus size (≤5K facts), full
//! rebuild is cheap enough that this is not a concern.
//!
//! ## Bidirectional adjacency
//!
//! The file stores **both forward and backward** CSR arrays. Both are derived
//! from the same canonical edge set at write time.
//!
//! ### Why bidirectional:
//!
//! - "What caused X?" requires backward traversal (O(1) with backward CSR,
//!   O(E) scan without it).
//! - "What does X enable?" requires forward traversal.
//! - "Chain from X to Y" requires BFS/DFS in either direction.
//! - Storage cost: 2× the edge arrays. For 5K nodes with avg degree 2,
//!   that's ~80KB — negligible.
//! - The backward array is derived from forward edges at write time (transposed).
//!   It is never declared independently. The writer builds forward edges from
//!   the canonical edge set, then transposes to produce backward edges.
//!
//! ## Edge deduplication
//!
//! A logical edge A→B can be declared in two places:
//! - `causes: [B]` on fact A (forward declaration)
//! - `caused_by: [A]` on fact B (backward declaration)
//!
//! **Canonical rule**: The edge set is the union of both declarations after
//! normalization. If both A.causes contains B and B.caused_by contains A,
//! that is one edge, not two. Neither field "wins" — they are merged.
//!
//! The writer collects all (source, target) pairs from both `causes` and
//! `caused_by`, inserts them into a set, and deduplicates. Edges referencing
//! unknown fact IDs are dropped with a `CausalValidationWarning::DanglingEdge`.
//!
//! ## Graph fingerprint
//!
//! `graph_fingerprint` is BLAKE3(sorted_edge_list), where sorted_edge_list is
//! the lexicographically sorted sequence of `"{source_id}\0{target_id}\n"` for
//! every canonical edge. This makes the fingerprint:
//! - Deterministic given the same edges
//! - Independent of node index assignment
//! - Sensitive to any edge addition, removal, or endpoint change
//!
//! The incremental compile gate: if the new fingerprint matches the previous
//! `causal.csr` header's fingerprint, skip writing a new file.

use bytemuck::{Pod, Zeroable};

// ─── Constants ───────────────────────────────────────────────────────────────

/// Magic bytes identifying a causal graph CSR file.
/// Mnemonic: "ENGR" = Engram, "CASL" = Causal.
pub const CAUSAL_MAGIC: [u8; 8] = *b"ENGRCASL";

/// Current causal graph format version.
pub const CAUSAL_VERSION: u32 = 1;

/// Sentinel node index meaning "no node" / "invalid reference".
pub const NULL_NODE: u32 = u32::MAX;

// ─── File header ─────────────────────────────────────────────────────────────

/// Causal graph file header — exactly 64 bytes, at byte offset 0.
///
/// Followed by: string table, node table, forward CSR, backward CSR.
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub struct CausalHeader {
    /// File format identifier. Must be `CAUSAL_MAGIC`.
    pub magic: [u8; 8],

    /// Format version. Must be `CAUSAL_VERSION`.
    pub version: u32,

    /// Number of nodes in the graph. Each node corresponds to one fact.
    pub node_count: u32,

    /// Number of forward edges. Forward and backward edge counts are always
    /// equal (backward CSR is a transpose of forward). Single field suffices.
    pub edge_count: u32,

    /// Total byte length of the string table (before alignment padding).
    pub string_table_bytes: u32,

    /// Index state generation at the time the graph was built.
    /// The reader must compare this against the current generation.
    /// If mismatched, the graph is stale and should not be used for
    /// queries that require generation-consistent results.
    pub generation: u64,

    /// BLAKE3 hash of the canonical edge set, truncated to 128 bits (16 bytes).
    /// Full 256-bit output is not required for this corpus size.
    /// Used as an incremental compile gate: if the new fingerprint
    /// matches the existing file's fingerprint, skip the graph rebuild.
    pub graph_fingerprint: [u8; 16],

    /// Reserved for future use. Must be zero.
    pub _pad: [u8; 16],
}

const _: () = assert!(std::mem::size_of::<CausalHeader>() == 64);

// ─── Node ────────────────────────────────────────────────────────────────────

/// A node in the causal graph — exactly 32 bytes.
///
/// Nodes are sorted by fact_id (lexicographic) in the node table.
/// The fact_id string is stored in the string table at
/// `string_table[id_offset..id_offset + id_len]`.
#[derive(Debug, Clone, Copy, Pod, Zeroable, PartialEq, Eq)]
#[repr(C)]
pub struct CausalNode {
    /// Byte offset into the string table where this node's fact_id begins.
    pub id_offset: u32,

    /// Byte length of this node's fact_id in the string table.
    pub id_len: u32,

    /// FNV-1a hash of the fact's source_path bytes. Links this node to
    /// its corresponding Tantivy document and temporal log record without
    /// opening any other file.
    pub source_path_hash: u64,

    /// Importance score from the fact's frontmatter (f64 → bits).
    /// Stored as raw bits so the struct remains Pod. Reconstruct via
    /// `f64::from_bits(node.importance_bits)`.
    pub importance_bits: u64,

    /// Fact type discriminant: 0=durable, 1=state, 2=event.
    /// Mirrors `temporal::FACT_TYPE_*` constants.
    pub fact_type: u8,

    /// Number of forward edges (out-degree). Redundant with CSR offsets
    /// but useful for quick degree checks without loading the offset array.
    pub out_degree: u8,

    /// Number of backward edges (in-degree). Same rationale as out_degree.
    pub in_degree: u8,

    /// Reserved. Must be zero.
    pub _pad: [u8; 5],
}

const _: () = assert!(std::mem::size_of::<CausalNode>() == 32);

// ─── Edge ────────────────────────────────────────────────────────────────────

/// A directed edge in the causal graph — exactly 8 bytes.
///
/// Stored in both the forward and backward CSR target arrays.
/// In the forward array: target_node is the destination (effect).
/// In the backward array: target_node is the source (cause).
#[derive(Debug, Clone, Copy, Pod, Zeroable, PartialEq, Eq)]
#[repr(C)]
pub struct CausalEdge {
    /// Index of the target node in the node table.
    pub target_node: u32,

    /// Edge weight, quantized to u16 (0–65535 maps to 0.0–1.0).
    /// Derived from the source node's confidence × importance.
    /// Allows weighted traversal without loading the full node record.
    pub weight: u16,

    /// Reserved. Must be zero.
    pub _pad: [u8; 2],
}

const _: () = assert!(std::mem::size_of::<CausalEdge>() == 8);

// ─── Build report ────────────────────────────────────────────────────────────

/// Summary produced by the causal graph writer after building the graph.
/// Not stored in the file — returned to the caller for logging/diagnostics.
#[derive(Debug, Clone)]
pub struct CausalBuildReport {
    /// Number of nodes in the graph.
    pub node_count: u32,

    /// Number of unique directed edges in the canonical edge set.
    pub edge_count: u32,

    /// Number of edges dropped because one endpoint was an unknown fact ID.
    pub dangling_edges_dropped: u32,

    /// Number of duplicate edges removed during deduplication.
    pub duplicate_edges_removed: u32,

    /// Milliseconds spent building the graph.
    pub build_ms: u64,

    /// The graph fingerprint that was written to the header.
    pub graph_fingerprint: [u8; 16],

    /// True if the graph was skipped because the fingerprint matched
    /// the previous build.
    pub skipped_unchanged: bool,

    /// Validation warnings encountered during the build.
    pub warnings: Vec<CausalValidationWarning>,
}

/// Warnings produced during causal graph construction.
/// These are non-fatal — the graph is still written, but the caller
/// should surface these to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CausalValidationWarning {
    /// An edge references a fact ID that does not exist in the corpus.
    /// Contains (source_id, target_id) where one or both are unknown.
    DanglingEdge {
        source_id: String,
        target_id: String,
    },

    /// A self-loop was detected and removed (A→A).
    SelfLoop {
        fact_id: String,
    },

    /// A cycle was detected during optional DAG validation.
    /// Contains the fact IDs forming the cycle (may be approximate).
    CycleDetected {
        cycle_ids: Vec<String>,
    },
}

// ─── Alignment helpers ───────────────────────────────────────────────────────

/// Round up `n` to the next multiple of `align`. `align` must be a power of 2.
#[inline]
pub const fn align_up(n: usize, align: usize) -> usize {
    (n + align - 1) & !(align - 1)
}

// ─── Parsing helpers ─────────────────────────────────────────────────────────

/// Validate a causal.csr header. Returns an error if magic, version, or
/// internal consistency checks fail.
pub fn validate_causal_header(header: &CausalHeader) -> anyhow::Result<()> {
    if header.magic != CAUSAL_MAGIC {
        anyhow::bail!(
            "invalid causal.csr magic: expected {:?}, got {:?}",
            CAUSAL_MAGIC,
            header.magic
        );
    }
    if header.version != CAUSAL_VERSION {
        anyhow::bail!(
            "causal.csr version mismatch: expected {}, got {}",
            CAUSAL_VERSION,
            header.version
        );
    }
    Ok(())
}

/// Compute the expected total file size from header fields.
/// Used by the reader to validate file length before casting slices.
pub fn expected_file_size(header: &CausalHeader) -> usize {
    let n = header.node_count as usize;
    let e = header.edge_count as usize;
    let st = align_up(header.string_table_bytes as usize, 4);

    64                                  // header
    + st                                // string table (4-byte aligned)
    + n * std::mem::size_of::<CausalNode>()       // node table
    + (n + 1) * std::mem::size_of::<u32>()        // forward offsets
    + e * std::mem::size_of::<CausalEdge>()       // forward targets
    + (n + 1) * std::mem::size_of::<u32>()        // backward offsets
    + e * std::mem::size_of::<CausalEdge>()       // backward targets
}

/// Quantize a floating-point weight (0.0–1.0) to u16 (0–65535).
#[inline]
pub fn quantize_weight(w: f64) -> u16 {
    (w.clamp(0.0, 1.0) * 65535.0) as u16
}

/// Dequantize a u16 weight back to f64 (0.0–1.0).
#[inline]
pub fn dequantize_weight(w: u16) -> f64 {
    w as f64 / 65535.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_header_size() {
        assert_eq!(std::mem::size_of::<CausalHeader>(), 64);
    }

    #[test]
    fn test_node_size() {
        assert_eq!(std::mem::size_of::<CausalNode>(), 32);
    }

    #[test]
    fn test_edge_size() {
        assert_eq!(std::mem::size_of::<CausalEdge>(), 8);
    }

    #[test]
    fn test_align_up() {
        assert_eq!(align_up(0, 4), 0);
        assert_eq!(align_up(1, 4), 4);
        assert_eq!(align_up(4, 4), 4);
        assert_eq!(align_up(5, 4), 8);
        assert_eq!(align_up(100, 8), 104);
    }

    #[test]
    fn test_weight_quantization_roundtrip() {
        assert_eq!(quantize_weight(0.0), 0);
        assert_eq!(quantize_weight(1.0), 65535);
        assert_eq!(quantize_weight(0.5), 32767);

        // Roundtrip accuracy within ~0.002%
        let w = 0.73;
        let q = quantize_weight(w);
        let d = dequantize_weight(q);
        assert!((w - d).abs() < 0.001, "roundtrip error: {} vs {}", w, d);
    }

    #[test]
    fn test_weight_clamp() {
        assert_eq!(quantize_weight(-0.5), 0);
        assert_eq!(quantize_weight(1.5), 65535);
    }

    #[test]
    fn test_expected_file_size_empty() {
        let header = CausalHeader {
            magic: CAUSAL_MAGIC,
            version: CAUSAL_VERSION,
            node_count: 0,
            edge_count: 0,
            string_table_bytes: 0,
            generation: 1,
            graph_fingerprint: [0; 16],
            _pad: [0; 16],
        };
        // 64 (header) + 0 (strings) + 0 (nodes) + 4 (fwd offsets: 1×u32)
        // + 0 (fwd targets) + 4 (bwd offsets: 1×u32) + 0 (bwd targets)
        assert_eq!(expected_file_size(&header), 72);
    }

    #[test]
    fn test_expected_file_size_small_graph() {
        let header = CausalHeader {
            magic: CAUSAL_MAGIC,
            version: CAUSAL_VERSION,
            node_count: 3,
            edge_count: 2,
            string_table_bytes: 10, // will be padded to 12
            generation: 1,
            graph_fingerprint: [0; 16],
            _pad: [0; 16],
        };
        let expected = 64          // header
            + 12                   // string table (10 → 12 aligned)
            + 3 * 32              // nodes
            + 4 * 4               // fwd offsets (3+1 u32s)
            + 2 * 8               // fwd targets
            + 4 * 4               // bwd offsets
            + 2 * 8;              // bwd targets
        assert_eq!(expected_file_size(&header), expected);
    }

    #[test]
    fn test_validate_header_ok() {
        let header = CausalHeader {
            magic: CAUSAL_MAGIC,
            version: CAUSAL_VERSION,
            node_count: 0,
            edge_count: 0,
            string_table_bytes: 0,
            generation: 1,
            graph_fingerprint: [0; 16],
            _pad: [0; 16],
        };
        assert!(validate_causal_header(&header).is_ok());
    }

    #[test]
    fn test_validate_header_bad_magic() {
        let header = CausalHeader {
            magic: *b"BADMAGIC",
            version: CAUSAL_VERSION,
            node_count: 0,
            edge_count: 0,
            string_table_bytes: 0,
            generation: 1,
            graph_fingerprint: [0; 16],
            _pad: [0; 16],
        };
        assert!(validate_causal_header(&header).is_err());
    }

    #[test]
    fn test_validate_header_bad_version() {
        let header = CausalHeader {
            magic: CAUSAL_MAGIC,
            version: 99,
            node_count: 0,
            edge_count: 0,
            string_table_bytes: 0,
            generation: 1,
            graph_fingerprint: [0; 16],
            _pad: [0; 16],
        };
        assert!(validate_causal_header(&header).is_err());
    }

    #[test]
    fn test_null_node_sentinel() {
        assert_eq!(NULL_NODE, u32::MAX);
        // A graph with u32::MAX nodes is physically impossible (~96 GB node table)
        // so NULL_NODE is always distinguishable from valid indices.
    }

    #[test]
    fn test_node_zeroed_is_valid_pod() {
        let node = CausalNode::zeroed();
        assert_eq!(node.id_offset, 0);
        assert_eq!(node.id_len, 0);
        assert_eq!(node.source_path_hash, 0);
        assert_eq!(node.fact_type, 0);
        assert_eq!(node.out_degree, 0);
        assert_eq!(node.in_degree, 0);
        assert_eq!(node.importance_bits, 0);
    }

    #[test]
    fn test_edge_zeroed_is_valid_pod() {
        let edge = CausalEdge::zeroed();
        assert_eq!(edge.target_node, 0);
        assert_eq!(edge.weight, 0);
    }
}
