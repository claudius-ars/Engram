# End-to-End Integration Tests

Shell-based integration tests that exercise the compiled `engram` binary
against a seed corpus of 10 Markdown files.

## Prerequisites

- Rust toolchain (to build the binary)
- `python3` or `jq` (for JSON validation in audit tests)
- macOS or Linux

## Running

```bash
# Build the binary
cargo build --release

# Run all E2E tests
./e2e/run.sh

# Or with a debug build
ENGRAM_BIN=target/debug/engram ./e2e/run.sh
```

## Test Cases

| # | Name | Description |
|---|------|-------------|
| 1 | Fresh compile | Full compile of seed corpus, verifies index creation |
| 2 | Malformed file skip | Compiler handles malformed frontmatter without crashing |
| 3 | Basic query | BM25 query returns relevant results |
| 4 | Expired fact filtering | Queries handle expired `state` facts gracefully |
| 5 | Causal query | Causal signal words trigger causal graph traversal |
| 6 | Curate | `curate --sync` creates a new fact and recompiles |
| 7 | Incremental compile | Adding a file and running `--incremental` indexes it |
| 8 | Access count tracking | Query hits generate access log entries |
| 9 | Policy deny | Bulwark deny rules are enforced at query time |
| 10 | Audit log creation | Operations produce NDJSON audit entries |
| 11 | Audit tampering detection | `--verify-audit` detects corrupted hash chains |
| 12 | Empty workspace | Compile handles empty context tree gracefully |

## Seed Corpus

The `corpus/` directory contains 10 `.md` files covering various fact types
(durable, state, event), causal relationships, an expired state fact, and
one intentionally malformed file. These are copied into each test's isolated
workspace under `.brv/context-tree/`.

## Isolation

Each test creates a fresh `mktemp -d` workspace. The seed corpus is copied
in, the binary runs against it, and the workspace is cleaned up afterward.
Tests do not share state.
