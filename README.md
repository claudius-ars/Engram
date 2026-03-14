# Engram

A knowledge compiler and query engine designed for AI agent memory that transforms Markdown files with YAML frontmatter into a BM25-searchable index with multi-tier caching, causal graph traversal, temporal queries, and governance controls.

---

## Overview

Engram takes a directory of `.md` files (the **context tree**), parses their YAML frontmatter, validates and normalizes the fields, and compiles them into a [Tantivy](https://github.com/quickwit-oss/tantivy) full-text search index. Queries pass through a multi-tier cache pipeline before hitting BM25 direct search, with optional causal graph expansion, temporal filtering, and LLM synthesis.



## Architecture

```
.brv/context-tree/*.md
        │
        ▼
   File Walker ──► Parser ──► Validator ──► Indexer ──► Tantivy Index
                                                            │
Query ──► Bulwark Policy ──► Cache Pipeline ──► BM25 ──► QueryResult
              │                  │
              │            Tier 0: Exact (MD5, 60s TTL)
              │            Tier 1: Fuzzy (Jaccard similarity)
              │            Tier 2: BM25 direct
              │            Tier 2.5: Causal + Temporal
              │            Tier 3: LLM synthesis (opt-in)
              │
              ▼
         Audit Log (.brv/audit/engram.log)
```

### Crate Dependency Graph

```
engram-core                  (leaf — schema, parsing, validation)
engram-bulwark               (leaf — policy engine, audit log)
    │                    │
    ▼                    ▼
engram-compiler ─────────┘  (indexing, compilation, curation)
                 │
                 ▼
engram-query                 (search, caching, causal/temporal queries)
    │
    ▼
engram-openclaw              (plugin interface, context formatting)
    │
    ▼
engram-cli                   (binary entry point)
```

## Installation

### Download binary (recommended)

Download the pre-built binary for your platform from the
[latest release](https://github.com/claudius-ars/Engram/releases/latest):

**macOS (Apple Silicon):**
```bash
curl -L https://github.com/claudius-ars/Engram/releases/latest/download/engram-macos-arm64 \
  -o ~/.local/bin/engram && chmod +x ~/.local/bin/engram
```

**macOS (Intel):**
```bash
curl -L https://github.com/claudius-ars/Engram/releases/latest/download/engram-macos-x86_64 \
  -o ~/.local/bin/engram && chmod +x ~/.local/bin/engram
```

**Linux (x86_64):**
```bash
curl -L https://github.com/claudius-ars/Engram/releases/latest/download/engram-linux-x86_64 \
  -o ~/.local/bin/engram && chmod +x ~/.local/bin/engram
```

Verify:
```bash
engram --help
```

### Build from source

Requires Rust 1.75+:

```bash
# Install from git
cargo install --git https://github.com/claudius-ars/Engram engram-cli

# Or clone and build
git clone https://github.com/claudius-ars/Engram.git
cd Engram
cargo build --release
cp target/release/engram ~/.local/bin/
```

### Prerequisites

- Optional: `ANTHROPIC_API_KEY` environment variable for LLM
  classification and Tier 3 synthesis

## Getting Started

Three steps to go from a fresh project to a working Engram workspace.

### Step 1 — Initialize a workspace

Navigate to your project and create the first fact. Engram initializes
the `.brv/` workspace automatically on first use:

```bash
cd your-project
engram curate --sync "Brief description of this project and its purpose"
```

This creates `.brv/context-tree/`, writes the fact as a `.md` file,
compiles the index, and makes the fact immediately queryable.

### Step 2 — Add more facts

Add facts by curating from the command line or by writing `.md` files
directly:

```bash
# Curate from the command line
engram curate --sync "We use Rust for all new services. Decision made Q1 2024."

# Or write a .md file and compile
cat > .brv/context-tree/api-versioning.md << 'EOF'
---
title: "API Versioning Policy"
factType: durable
tags: [api, versioning]
---

## Raw Concept

All APIs use URL path versioning (/v1/, /v2/). Deprecation window is
12 months. Breaking changes require a new major version.
EOF
engram compile
```

### Step 3 — Query the index

```bash
engram query "what is our versioning policy"
```

From here, run `engram compile --watch` to keep the index up to date
as you add facts, or install the Claude Code plugin to get automatic
memory injection in every coding session (see
[Claude Code Integration](#claude-code-integration)).

## Usage

### Compile

Compile the context tree into a searchable index:

```bash
# Full compile
engram compile

# Incremental compile (reindex only changed files)
engram compile --incremental

# Watch mode (recompile on file changes)
engram compile --watch

# Run LLM classification on unclassified facts
engram compile --classify
```

### Query

Search the compiled index:

```bash
engram query "kubernetes deployment strategies"
```

Output includes hit count, cache tier, execution time, and ranked results with scores and source paths.

### Audit Verification

```bash
# Verify the audit log hash chain
engram query --verify-audit

# Verify a specific audit log file
engram query --verify-audit --log .brv/audit/engram.log.20240315-120000
```

### Curate

Create a new fact from a summary:

```bash
# Async (background compile)
engram curate "Redis switched from BSD to dual-license in March 2024"

# Sync (blocking compile, immediately queryable)
engram curate --sync "The team adopted Rust for the data pipeline"
```

## Fact Format

Facts are Markdown files with YAML frontmatter stored in `.brv/context-tree/`:

```yaml
---
title: "Kubernetes Pod Scheduling"
factType: durable
tags:
  - kubernetes
  - infrastructure
keywords:
  - scheduling
  - pod-affinity
domainTags:
  - infra:k8s
confidence: 0.95
importance: 0.8
recency: 1.0
causedBy:
  - cluster-migration-2024
---

## Raw Concept

Kubernetes pod scheduling uses a two-phase process: filtering
(which nodes can run the pod) and scoring (which node is best).
Pod affinity rules allow co-locating related workloads.
```

### Fact Types

| Type | Field Value | Description |
|------|-------------|-------------|
| **Durable** | `durable` | Long-lived knowledge. No expiry. Default when `factType` is omitted. |
| **State** | `state` | Current system state. May become stale. Supports `validUntil` for expiry. |
| **Event** | `event` | Point-in-time occurrence. Should have `eventSequence` for ordering. |

### Supported Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `title` | string | None | Fact title. 3.0x boost in BM25 scoring. |
| `tags` | list | `[]` | Searchable tags. 2.0x boost. |
| `keywords` | list | `[]` | Searchable keywords. 2.0x boost. |
| `domainTags` | list | `[]` | Namespaced tags (e.g., `infra:k8s`). 1.5x boost. |
| `factType` | string | `durable` | One of: `durable`, `state`, `event`. |
| `confidence` | float | `1.0` | Confidence weight for compound scoring. |
| `importance` | float | `1.0` | Importance weight. Bumped by access tracking. |
| `recency` | float | `1.0` | Recency weight for compound scoring. |
| `maturity` | float | `1.0` | Stored but not used in scoring. |
| `validUntil` | datetime | None | Expiry timestamp for state facts. |
| `causedBy` | list | `[]` | Upstream fact IDs (causal graph). |
| `causes` | list | `[]` | Downstream fact IDs (causal graph). |
| `related` | list | `[]` | Related fact IDs. |
| `eventSequence` | integer | None | Ordering for event facts. |
| `createdAt` | datetime | None | Creation timestamp. |
| `updatedAt` | datetime | None | Last update timestamp. |
| `accessCount` | integer | `0` | Access counter (managed by the system). |
| `updateCount` | integer | `0` | Update counter. |

All field names accept both `snake_case` and `camelCase` (e.g., `factType` / `fact_type`, `causedBy` / `caused_by`).

## Query Pipeline

Queries pass through multiple tiers. Each tier is tried in order; the first hit is returned.

### Tier 0 — Exact Cache

HashMap keyed by MD5 fingerprint of the query string. 60-second TTL. Generation-aware — entries from prior index generations are rejected. Bypassed when the dirty flag is set.

### Tier 1 — Fuzzy Cache

Jaccard similarity on normalized token sets. Threshold: 0.6 (configurable). Same TTL and generation semantics as Tier 0.

### Tier 2 — BM25 Direct

Opens the Tantivy index and runs a multi-field query with field boosts:

| Field | Boost |
|-------|-------|
| `title` | 3.0x |
| `tags`, `keywords` | 2.0x |
| `domain_tags` | 1.5x |
| `body`, `id` | 1.0x |

Results are scored with a compound formula: `bm25_norm * confidence * importance * recency`.

### Tier 2.5a — Causal Query

Triggered when the query contains causal signal words (`caused by`, `leads to`, `why did`, etc.). Traverses the causal graph up to `causal_max_hops` (default 3, max 6) to find upstream and downstream facts.

### Tier 2.5b — Temporal Query

Triggered when the query contains temporal signal words (`current`, `latest`, `since`, `history`, etc.). Classifies the query into one of three patterns:

- **CurrentState** — prioritizes `state` facts with recent timestamps
- **SinceTimestamp** — filters facts updated after a threshold
- **EventHistory** — returns `event` facts ordered by `eventSequence`

### Tier 3 — LLM Synthesis (Opt-in)

Fires when enabled and the best BM25 score falls below `tier3_score_threshold`. Sends the top-N fact bodies to an LLM (Claude) for synthesis. Requires `ANTHROPIC_API_KEY`.

## Configuration

### Workspace Configuration

Create `.brv/engram.toml` in your workspace root:

```toml
[query]
score_threshold = 0.85
score_gap = 0.10
jaccard_threshold = 0.60
exact_cache_ttl_secs = 60
causal_max_hops = 3

[query.tier3]
enabled = false
top_n = 5
score_threshold = 0.75

[compile]
classify = false
max_tokens_per_compile = 10000

[access_tracking]
enabled = true
importance_delta = 0.001

[audit]
max_log_bytes = 52428800   # 50 MB; set to 0 to disable rotation
# siem_endpoint  = "https://siem.example.com/ingest"
# siem_token_env = "SIEM_API_TOKEN"   # env var holding bearer token
# siem_required  = false              # true = fail startup if SIEM unreachable
```

### Policy Configuration

Create `bulwark.toml` for access control:

```toml
[[rules]]
name = "allow-reads"
effect = "allow"
access_type = "read"

[[rules]]
name = "block-untrusted"
effect = "deny"
agent = "untrusted-*"
reason = "untrusted agent"

[[rules]]
name = "deny-llm"
effect = "deny"
access_type = "llm_call"
reason = "LLM calls disabled in this workspace"

[[rules]]
name = "allow-rest"
effect = "allow"
```

Rules are evaluated first-match. If no rule matches, the default is **deny** (fail-closed). Supported fields:

| Field | Values | Default |
|-------|--------|---------|
| `name` | any string | required |
| `effect` | `"allow"` or `"deny"` | required |
| `access_type` | `"read"`, `"write"`, `"llm_call"`, `"*"` | `"*"` |
| `agent` | exact string or prefix glob (`"agent-*"`) | `"*"` |
| `reason` | any string | auto-generated |
| `operations` | `"query"`, `"compile"`, `"curate"`, `"*"` | `[]` (all) |
| `domain_tags_allow` | list of glob patterns | `[]` (no restriction) |
| `domain_tags_deny` | list of glob patterns | `[]` (no exclusions) |
| `fact_types` | `"durable"`, `"state"`, `"event"`, `"*"` | `[]` (all) |

**Rule matching semantics:** A rule matches only when all specified
conditions hold. Empty lists mean no restriction on that dimension.
`domain_tags_deny` patterns cause a rule to be skipped (not a terminal
deny) — the next rule in declaration order then evaluates. No match →
deny (fail-closed).

The policy file is hot-reloaded every 30 seconds. On Unix, sending
`SIGHUP` to the process triggers an immediate reload. Changes take
effect without restarting.

### Domain Ontology

Create an `ontology.json` for domain-specific term expansion:

```json
{
  "version": 1,
  "namespaces": {
    "infra": {
      "label": "Infrastructure",
      "terms": {
        "k8s": {
          "parent": "orchestration",
          "related": ["docker", "containerization"],
          "equivalent": ["kubernetes"]
        }
      }
    }
  }
}
```

Reference it in `engram.toml`:

```toml
[ontology]
file = ".brv/ontology.json"
expansion_depth = 1          # 0 = no expansion; 1 = direct neighbors (default); max = 3
```

At compile time, `domain_tags` are validated against registered namespaces. At query time, tokens are expanded depth-1 using parent, related, and equivalent terms.

## Access Tracking

When enabled, every query hit appends an entry to `.brv/index/access.log`. On the next compile, these entries are tallied and written back to the index:

- `access_count` accumulates across compile cycles (previous count read from index + new tally)
- `importance` is bumped by `importance_delta` per new access (only current-cycle accesses affect importance)
- The access log is truncated after each successful compile
- Stale log entries (from generations older than N-2) are filtered out

## Audit Log

When a `BulwarkHandle` is created with an audit directory, every policy decision is recorded to `.brv/audit/engram.log` as NDJSON with a SHA-256 hash chain:

```json
{"ts_ms":1710288000000,"agent_id":"agent-1","operation":"query","access_type":"Read","decision":"allow","prev_hash":"0000...0000"}
{"ts_ms":1710288001000,"agent_id":"agent-2","operation":"compile","access_type":"Write","decision":"deny","reason":"restricted","rule_name":"deny-writes","prev_hash":"a3f2..."}
```

Each entry's `prev_hash` is the SHA-256 of the complete previous line (including newline). The chain can be verified programmatically:

```rust
use engram_bulwark::verify_audit_chain;
let count = verify_audit_chain(Path::new(".brv/audit/engram.log"))?;
```

Tampered entries produce `ChainError::HashMismatch` at the entry following the modification.

## Directory Structure

```
.brv/
├── context-tree/          # Source .md files (your facts)
│   └── *.md
├── index/
│   ├── tantivy/           # Tantivy index files (derived artifact)
│   ├── manifest.bin       # Bincode-serialized manifest
│   ├── state              # JSON with generation counter and dirty flag
│   ├── access.log         # NDJSON access log (truncated on compile)
│   ├── temporal.log        # Temporal backfill log
│   ├── causal.csr         # Binary causal graph (derived artifact)
├── audit/
│   └── engram.log         # Append-only audit log with hash chain
├── engram.toml            # Workspace configuration
└── bulwark.toml           # Policy rules
```

The Tantivy index is a derived artifact — the `.md` source files are the source of truth. The index can be safely deleted and rebuilt with `engram compile`.

## ByteRover Compatibility

Engram accepts ByteRover-format `.md` files without modification. Existing
ByteRover corpora can be pointed at Engram and compiled directly — no
frontmatter changes required.

The following field name aliases are supported transparently:

| ByteRover (camelCase) | Engram canonical |
|-----------------------|-----------------|
| `factType` | `fact_type` |
| `causedBy` | `caused_by` |
| `validUntil` | `valid_until` |
| `accessCount` | `access_count` |
| `updateCount` | `update_count` |
| `eventSequence` | `event_sequence` |
| `domainTags` | `domain_tags` |
| `createdAt` | `created_at` |
| `updatedAt` | `updated_at` |

Both forms are accepted in frontmatter. Engram normalizes to snake_case
internally.

**What carries over without changes:**
- Fact content and body text
- All frontmatter fields listed above
- Causal relationships (`causedBy` / `caused_by`)
- Tags, keywords, confidence, importance, and recency weights
- `validUntil` expiry on state facts

**What does not carry over:**
- ByteRover query cache entries — Engram builds its own index on first
  compile; run `engram compile` to populate it
- Any ByteRover-specific plugin configuration — Engram has its own
  `engram.toml` format

## Claude Code Integration

Engram ships with a Claude Code plugin that gives Claude persistent,
project-scoped memory. Once installed, every prompt automatically retrieves
relevant facts from the knowledge base and injects them into the
conversation context.

### Quick Start

Add the hooks to `.claude/settings.json` in your project root
(create the file if it doesn't exist):

```json
{
  "hooks": {
    "UserPromptSubmit": [
      {
        "matcher": ".*",
        "hooks": [
          {
            "type": "command",
            "command": "/absolute/path/to/engram-claudecode/hooks/user_prompt_submit.sh"
          }
        ]
      }
    ],
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "/absolute/path/to/engram-claudecode/hooks/session_end.sh"
          }
        ]
      }
    ]
  }
}
```

Replace `/absolute/path/to/engram-claudecode` with the actual path
to your Engram installation. Absolute paths are required — Claude Code
resolves hook commands from its own working directory, not the project
root.

Restart Claude Code after editing `settings.json` for the hooks to
take effect.

### What the Plugin Provides

| Component | Trigger | What it does |
|-----------|---------|--------------|
| **Retrieval hook** | Every prompt | Queries the index, injects matching facts as context |
| **Session end hook** | Session stop | Reminds user to curate new facts |
| **Memory recall skill** | On demand | Query or save facts explicitly via `engram query` / `engram curate` |

### Requirements

- `engram` binary in PATH — see [Installation](#installation)
- An initialized `.brv/` workspace with compiled index

See [`engram-claudecode/README.md`](engram-claudecode/README.md) for full
installation and configuration details.

## Testing

```bash
# Run all tests
cargo test

# Run a specific test suite
cargo test --test test_pipeline
cargo test --test test_bulwark
cargo test --test test_audit_chain

# Run with clippy
cargo clippy --all-targets -- -D warnings
```

The test suite includes 513 tests across 16 integration test files and unit tests in each crate.

## License

See [LICENSE](LICENSE) for details.
