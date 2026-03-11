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
| 19 | maturity | f64 | STORED | ByteRover maturity score. |
| 20 | access_count | u64 | STORED | ByteRover access counter. |
| 21 | update_count | u64 | STORED | ByteRover update counter. |

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
omitted (ByteRover compatibility).

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

## ByteRover Compatibility

Engram maintains full backward compatibility with ByteRover-format
`.md` files. Existing ByteRover corpora work without modification.

### Supported ByteRover Fields

| ByteRover Field | Type | Engram Mapping | Default |
|---|---|---|---|
| title | string | title | None |
| tags | list | tags (space-joined for TEXT index) | [] |
| keywords | list | keywords (space-joined for TEXT index) | [] |
| importance | float | importance (FAST f64) | 1.0 |
| recency | float | recency (FAST f64) | 1.0 |
| maturity | float | maturity (STORED f64) | 1.0 |
| accessCount | integer | access_count (STORED u64) | 0 |
| updateCount | integer | update_count (STORED u64) | 0 |
| related | list | related (STORED JSON string) | [] |

### camelCase Aliases

ByteRover uses camelCase field names. The serde `#[serde(alias)]`
attribute handles these transparently:

- `accessCount` → `access_count`
- `updateCount` → `update_count`
- `factType` → `fact_type` (Engram-native, also aliased)
- `eventSequence` → `event_sequence` (Engram-native)
- `validUntil` → `valid_until` (Engram-native)
- `createdAt` → `created_at` (Engram-native)
- `updatedAt` → `updated_at` (Engram-native)
- `causedBy` → `caused_by` (Engram-native)
- `domainTags` → `domain_tags` (Engram-native)

### Behavioral Differences

- Missing `factType`: defaults to `durable`
- Missing `confidence`: defaults to `1.0`
- Missing `importance`/`recency`: defaults to `1.0`
- Missing `maturity`: defaults to `1.0`
- ByteRover `maturity` is stored but not used in compound scoring
- ByteRover `accessCount`/`updateCount` are stored but not used in
  compound scoring

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

## Phase 2+ Extension Points

The following items are explicitly deferred from Phase 1:

| Item | Current State | Target Phase |
|---|---|---|
| Tier 1 TTL | Hardcoded 60s | Phase 2 (configurable) |
| Cross-file caused_by validation | Not validated | Phase 3 |
| domain_tags namespace validation | Not validated | Phase 4 |
| Bulwark real policy engine | Stub (allow-all) | Phase 4 |
| ExactCache/FuzzyCache thread safety | Single-threaded | Phase 4 |
| Schema migration logic | Wipe-and-rebuild | Phase 2 |
| Manifest migration logic | Discard-and-regen | Phase 2 |
| SCORE_THRESHOLD configurability | Hardcoded 0.85 | Phase 2 |
| SCORE_GAP configurability | Hardcoded 0.1 | Phase 2 |
| JACCARD_THRESHOLD configurability | Hardcoded 0.6 | Phase 2 |
| Incremental indexing | Full rebuild every compile | Phase 2 |
| Watch mode | Not implemented | Phase 3 |
| QueryHit field expansion | 5 stored fields not in QueryHit | Phase 2 |

## Known Limitations

- **Full index rebuild on every compile.** The indexer deletes all
  documents and re-indexes from scratch. This is O(n) in corpus size
  and intentional for Phase 1 correctness. Incremental indexing is
  deferred to Phase 2.

- **No watch mode.** The query engine does not monitor the filesystem
  for changes. Callers must trigger recompilation explicitly.

- **FuzzyCache eviction is O(n).** At `max_entries = 100`, the cache
  scans all entries linearly on insert when full. This is acceptable
  for Phase 1 but should be replaced with LRU in Phase 2 if
  max_entries increases.

- **Single-threaded caches.** ExactCache and FuzzyCache are not
  `Send` or `Sync`. The OpenClaw plugin owns them in a single
  thread. Phase 4 must wrap them in `Mutex` or use concurrent maps
  if multi-threaded access is needed.

- **Body is not retrievable.** The body field is indexed as TEXT-only
  (no STORED flag) for full-text search. It cannot be returned in
  query results. This is intentional — body content can be read from
  the source `.md` file via `source_path`.
