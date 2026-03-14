# Engram Phase 1 Architecture

## Crate Dependency Graph

```
engram-core          (leaf — no internal deps)
engram-bulwark       (leaf — no internal deps)
    │                    │
    ▼                    ▼
engram-compiler ─────────┘
    │         │
    ▼         │
engram-query ─┘
    │
    ▼
engram-openclaw
    │
    │   engram-compiler ──► engram-query
    │         │                  │
    ▼         ▼                  ▼
engram-cli ───┘──────────────────┘
```

**Leaf crates (no internal dependencies):**
- `engram-core` — Schema definitions, frontmatter parsing, validation
- `engram-bulwark` — Governance interface (stub in Phase 1)

**Mid-tier crates:**
- `engram-compiler` — depends on core, bulwark
- `engram-query` — depends on core, bulwark

**Integration crates:**
- `engram-openclaw` — depends on query, bulwark
- `engram-cli` — depends on core, bulwark, compiler, query

## Pipeline Overview

The Engram pipeline transforms raw `.md` files with YAML frontmatter
into a BM25-searchable Tantivy index, then serves queries through a
three-tier cache pipeline.

### Compilation Pipeline

```
.brv/context-tree/*.md
        │
        ▼
   File Walker        (walker.rs)
   Discovers all .md files recursively
        │
        ▼
   Parser              (parser.rs)
   Extracts YAML frontmatter via serde_yaml
   Splits --- delimiters, parses RawFrontmatter
        │
        ▼
   Validator           (core/validation.rs)
   RawFrontmatter → FactRecord
   Applies defaults, type coercion, warnings
   Derives fact ID from context-tree-relative path
        │
        ▼
   Indexer             (indexer.rs)
   FactRecord → Tantivy document
   Full rebuild: delete all, write all, commit
   23-field schema with TEXT, FAST, STORED flags
        │
        ├──► Schema version file (.brv/index/tantivy/engram_schema_version)
        ├──► Manifest (.brv/index/manifest.bin) — bincode-serialized summary
        └──► State file (.brv/index/state) — JSON with generation counter
```

### Query Pipeline

```
Query string
     │
     ▼
Bulwark policy check ──► deny → Err(PolicyDenied)
     │ allow
     ▼
Read state file (generation, dirty flag)
     │
     ▼
MD5 fingerprint of query string
     │
     ├──► Tier 0: Exact Cache
     │    HashMap keyed by MD5 fingerprint
     │    60s TTL, generation-aware, dirty-bypass
     │    Hit → return cached QueryResult
     │
     ├──► Tier 1: Fuzzy Cache
     │    Jaccard similarity on normalized token sets
     │    Threshold 0.6, picks best match above threshold
     │    60s TTL, generation-aware, dirty-bypass
     │    Hit → return cached QueryResult
     │
     └──► Tier 2: BM25 Direct
          Open Tantivy index, build multi-field query
          Field boosts: title 3.0x, tags/keywords 2.0x,
                        domain_tags 1.5x, body/id 1.0x
          Compound scoring: bm25_norm * confidence * importance * recency
          Read FAST fields via segment column readers
          Insert result into both caches
     │
     ▼
QueryResult { hits: Vec<QueryHit>, meta: QueryMeta }
     │
     ▼
Formatter (openclaw/formatter.rs)
     │
     ▼
Markdown context block with sentinel strings
```

## Tantivy Schema

23 fields across three storage classes:

| # | Field | Type | Flags | Purpose |
|---|-------|------|-------|---------|
| 1 | title | text | TEXT, STORED | Fact title. 3.0x boost in BM25. |
| 2 | body | text | TEXT | Markdown body. 1.0x boost. Not retrievable. |
| 3 | tags | text | TEXT, STORED | Space-separated tags. 2.0x boost. |
| 4 | keywords | text | TEXT, STORED | Space-separated keywords. 2.0x boost. |
| 5 | domain_tags | text | TEXT, STORED | Namespaced tags (e.g., infra:k8s). 1.5x boost. |
| 6 | id | text | TEXT, STORED | Fact ID derived from path. 1.0x boost. |
| 7 | importance | f64 | FAST | Compound scoring weight. Default 1.0. |
| 8 | recency | f64 | FAST | Compound scoring weight. Default 1.0. |
| 9 | confidence | f64 | FAST | Compound scoring weight. Default 1.0. |
| 10 | fact_type_int | u64 | FAST | 0=durable, 1=state, 2=event. |
| 11 | valid_until_ts | i64 | FAST | Expiry timestamp. NULL_TIMESTAMP if none. |
| 12 | event_sequence | i64 | FAST | Event ordering. NULL_TIMESTAMP if none. |
| 13 | created_at_ts | i64 | FAST | Creation timestamp. NULL_TIMESTAMP if none. |
| 14 | updated_at_ts | i64 | FAST | Last update timestamp. NULL_TIMESTAMP if none. |
| 15 | source_path | text | STORED | Original .md file path. |
| 16 | caused_by | text | STORED | JSON array of upstream fact IDs. |
| 17 | causes | text | STORED | JSON array of downstream fact IDs. |
| 18 | related | text | STORED | JSON array of related fact IDs. |
| 19 | maturity | f64 | STORED | Maturity score (stored, not used in scoring). |
| 20 | access_count | u64 | STORED | Access counter. |
| 21 | update_count | u64 | STORED | Update counter. |

**NULL_TIMESTAMP** = `i64::MIN` — sentinel for missing timestamps.
Using 0 would conflict with Unix epoch (1970-01-01).

**FAST fields** use columnar storage accessed via segment column
readers (`searcher.segment_reader(addr.segment_ord).fast_fields()`).
They are NOT in the row store and cannot be read via `doc()`.

**STORED fields** are in the row store and retrieved via
`searcher.doc(addr)`.

## Cache Invalidation Contract

Two mechanisms prevent stale cache results:

### 1. Dirty Flag (Immediate Bypass)

When `state.dirty == true` (new facts curated but not yet compiled):
- Tier 0 exact cache: `get()` returns `None` regardless of TTL
- Tier 1 fuzzy cache: `get()` returns `None` regardless of TTL
- Queries always fall through to Tier 2 (BM25 direct)

The dirty flag is set by `curate()` and cleared by
`compile_context_tree()`.

### 2. Generation Counter (Stale Entry Rejection)

Each compile increments `state.generation` by 1. Cache entries
store the generation at insertion time.

- Tier 0: If `entry.generation != current_generation`, miss
- Tier 1: If `entry.generation != current_generation`, miss

This catches the case where the index was recompiled (clearing
dirty) but cached results are from a previous generation.

### Phase 4 Extension Point

Phase 4 will add explicit `invalidate_all()` calls triggered by
Bulwark governance events (e.g., policy change invalidates cached
results that may no longer be authorized).

## Fact Types

### Durable (factType: durable)

Long-lived knowledge. No expiry. Default type when `factType` is
omitted.

Validation: warns if `valid_until` is set (durable facts should
not expire).

### State (factType: state)

Current system state. May become stale. Typically has higher
recency weight.

Validation: no special rules beyond standard field validation.

### Event (factType: event)

Point-in-time occurrence. Should have `eventSequence` for ordering.

Validation: warns if `eventSequence` is missing (events without
sequence cannot be ordered).

### FactType Integer Encoding

Stored as `fact_type_int` FAST field: 0=durable, 1=state, 2=event.
Read back via segment column reader and converted to string in
QueryHit.

### Fields Stored But Not in QueryHit

Five fields are correctly parsed, validated, and written to the
Tantivy index but are NOT read back into QueryHit:

- `keywords` — indexed as TEXT (searchable), STORED, but not in QueryHit
- `maturity` — STORED only, not in QueryHit
- `access_count` — STORED only, not in QueryHit
- `update_count` — STORED only, not in QueryHit
- `related` — STORED as JSON, not in QueryHit

These fields are preserved in the index for future use. Phase 2
may expose them in QueryHit if downstream consumers need them.

## Schema and Manifest Versioning

### Schema Version

File: `.brv/index/tantivy/engram_schema_version`

`CURRENT_SCHEMA_VERSION = 1`

On compile, the indexer checks the version file:
- Missing: write current version, proceed normally
- Matches: proceed normally
- Differs or corrupt: wipe the tantivy directory, rebuild from
  scratch, write new version file

This automatic wipe-and-rebuild is safe because the `.md` source
files are the source of truth — the Tantivy index is a derived
artifact.

### Manifest Version

File: `.brv/index/manifest.bin`

`MANIFEST_VERSION = 1`

The manifest uses a `ManifestEnvelope` wrapper:
```
ManifestEnvelope { version: u32, entries: Vec<ManifestEntry> }
```

On read, if `envelope.version != MANIFEST_VERSION`, the reader
returns `ManifestError::VersionMismatch`. Callers treat this the
same as a missing manifest — it will be regenerated on next compile.

## Phase 4 Architecture

Phase 4 added schema v3, access count write-back, Tier 3 LLM
pre-fetch, domain ontology, and Bulwark policy types. This section
documents the key decisions and rationale.

### Non-Reversible Decisions (Phase 4)

**NRD-18: Tier 3 uses blocking HTTP (`reqwest::blocking`).** *(Closed — accepted.)*
The query pipeline is synchronous. Introducing an async runtime
solely for one HTTP call adds complexity with no benefit for the
CLI use case. Switching to async later requires either making the
full query pipeline async (large refactor) or spinning a tokio
runtime inside `run_tier3()` (contained but ugly).

**NRD-19: Access count write-back uses pre-write FactRecord mutation.**
The `body` field is `TEXT`-only in the Tantivy schema — not `STORED`.
Post-write Tantivy document patching is therefore impossible: you
cannot reconstruct a document from the index because the body is
not retrievable. Instead, access counts from the NDJSON access log
are applied to `FactRecord` structs before they are written to the
index. This pre-write mutation happens in `compile_context_tree_with_config()`
at Step 2c, before the Tantivy `IndexWriter::write()` call.

**NRD-20: `access.log` is gitignored; audit log is not.**
The access log (`.brv/index/access.log`) is an operational artifact
truncated after each compile — it is gitignored. A future audit log
(`audit/engram.log`) would be a compliance record: append-only,
hash-chained, and committed to version control. The two-file pattern
separates operational state from compliance records.

### Schema v3

`CURRENT_SCHEMA_VERSION = 3`

Schema v3 added two fields beyond the original 21:

| # | Field | Type | Flags | Purpose |
|---|-------|------|-------|---------|
| 22 | source_path_hash | u64 | FAST | FNV-1a hash of source_path for O(1) enrichment |
| 23 | importance | f64 | FAST, STORED | Upgraded from FAST-only to support pre-write mutation |

The `source_path_hash` FAST field and the `DocAddressMap` (`HashMap<u64, DocAddress>`)
enable O(1) document lookup in `enrich_hit()`. The map is built per-Searcher
snapshot at query time (not at index-open time — `DocAddress` values encode
segment ordinals specific to the Searcher they were built from). A fallback
O(N) segment scan is preserved for pre-v3 schemas that lack the FAST column.

### Access Count Write-Back

```
Query → append to .brv/index/access.log (NDJSON, one entry per hit)
Compile → tally_access_log() → apply_access_counts() → write Tantivy → truncate log
```

The access log uses generation-aware filtering: entries from generation
N-2 or older are skipped during tally at generation N. This prevents
stale log entries (surviving a failed truncation) from inflating counts.

Access counts are **cumulative across compile cycles**. At Step 2c of
`compile_context_tree_with_config()`, the compiler opens the previously
committed Tantivy index read-only and reads each fact's existing
`access_count` via FAST column readers. These previous counts are added
to the current-cycle delta from `tally_access_log()` before writing the
new index. The `importance` field (FAST + STORED in schema v3) is
similarly preserved across compiles.

### Tier 3 LLM Pre-fetch

Tier 3 fires after Tier 2.5b (temporal) when:
1. `config.tier3.enabled == true` (opt-in)
2. Best BM25 score < `score_threshold` (0.75 default)
3. Bulwark allows `AccessType::LlmCall`
4. `ANTHROPIC_API_KEY` is set

The LLM reads fact bodies from the filesystem (not from Tantivy —
same reason as NRD-19: body is not STORED). Source files for the
top-N hits are read, frontmatter stripped, and bodies truncated to
500 chars each. The synthetic hit has `source_path = "<llm:tier3>"`
as an unambiguous sentinel.

### Domain Ontology

The ontology subsystem operates at two points:

**Compile time**: Each fact's `domain_tags` are validated against
registered namespaces. Unknown terms in registered namespaces emit
a WARN. The fact is always indexed regardless.

**Query time**: Query tokens are expanded depth-1 using parent,
related, and equivalent terms from the ontology. Expanded tokens
are joined as a string and passed to Tantivy's `QueryParser` (not
a manual `BoolQuery`). This works because the QueryParser handles
multi-token strings with additive scoring — expanded terms increase
recall without requiring explicit OR clause construction.

All ontology code paths are gated on `ontology.is_some()` and
`!ontology.is_empty()`. Absent `ontology.json` produces zero
overhead and byte-for-byte identical results.

### Bulwark Policy Engine

Phase 4 added `AccessType::LlmCall` to the policy type system
and uses `BulwarkHandle` at three enforcement points:
- `query()` — checks `AccessType::Read` before any index access
- `compile_context_tree()` — checks `AccessType::Write` before indexing
- `tier3::run_tier3()` — checks `AccessType::LlmCall` before API call

## Phase 5 Architecture

Phase 5 replaced the Bulwark stub with a real TOML-backed policy
engine, added an append-only SHA-256 hash-chained audit log, and
introduced causal/temporal query tiers.

### Bulwark Policy Engine (Phase 5)

`BulwarkHandle` now has three construction modes:
- `new_stub()` — allow-all (tests, default)
- `new_denying()` — deny-all (tests)
- `new_from_config(policy_path, audit_dir)` — TOML-backed with hot-reload

**Policy rules** are loaded from `bulwark.toml` as `PolicyFile`
(TOML-deserialized `Vec<PolicyRule>`). Each rule matches on
`access_type`, `fact_id`, and `agent_id` using glob patterns
(`*`, `prefix*`, exact). Evaluation is first-match with a
fail-closed default deny when no rule matches.

**Phase 5 scope**: `PolicyRule` currently matches on `access_type`,
`fact_id`, and `agent_id`. The `operation` field on `PolicyRequest`
is not yet used for rule matching — it is recorded in audit events
for forensic use. Operation-based matching is a Phase 6 candidate.

**Hot-reload**: A background thread polls the policy file every 30s,
comparing file content with the previous read. On change, the
`PolicyState` behind `Arc<RwLock<_>>` is swapped atomically.

**Audit log**: When `audit_dir` is provided, `BulwarkHandle` writes
NDJSON events to `audit/engram.log`. Each entry includes a SHA-256
`prev_hash` of the preceding line, forming a tamper-evident chain.
File locking uses `fs2` exclusive locks. The `audit()` method is
non-fatal: write failures are logged to stderr but never propagated.

**Known limitation — `duration_ms`**: The `duration_ms` field in
`AuditEvent` is currently always 0 at all three enforcement points.
The policy check itself is sub-microsecond; the field exists for
future use when end-to-end operation timing is added.

### QueryHit.answer Field

`QueryHit` (defined in `engram-query::result`) has an `answer: Option<String>`
field for LLM-synthesized responses. Tier 3 populates this field
directly instead of encoding the answer as a `"synthesis:"` tag prefix.
The field is `#[serde(default, skip_serializing_if = "Option::is_none")]`
for backward-compatible JSON serialization — old JSON without `answer`
deserializes cleanly to `None`.

## Remaining Extension Points

| Item | Current State | Target Phase |
|---|---|---|
| Tier 1 TTL | Hardcoded 60s | Phase 5 (configurable) |
| Bulwark operation-based matching | Not yet in rule evaluation | Phase 6 |
| ExactCache/FuzzyCache thread safety | Single-threaded | Phase 5 |
| Schema migration logic | Wipe-and-rebuild | Phase 5 |
| O(1) temporal enrichment wiring | DocAddressMap built, used in causal/temporal | Done |

## Known Limitations

- **FuzzyCache eviction is O(n).** At `max_entries = 100`, the cache
  scans all entries linearly on insert when full. This is acceptable
  but should be replaced with LRU if max_entries increases.

- **Single-threaded caches.** ExactCache and FuzzyCache are not
  `Send` or `Sync`. The OpenClaw plugin owns them in a single
  thread.

- **Body is not retrievable from Tantivy.** The body field is indexed
  as TEXT-only (no STORED flag). It cannot be returned in query
  results. Body content must be read from the source `.md` file via
  `source_path`. This affects Tier 3 (reads filesystem) and prevents
  post-write Tantivy document patching (NRD-19).

- **`duration_ms` is always 0.** The audit event `duration_ms` field
  is populated as 0 at all enforcement points. End-to-end operation
  timing has not been wired yet.

- **`QueryHit` lives in `engram-query`**, not `engram-core`. It is
  defined in `crates/engram-query/src/result.rs`. Downstream crates
  that need `QueryHit` depend on `engram-query`.
