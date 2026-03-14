# Engram Retrieval Quality Eval

Measures whether Engram returns **relevant** results, not just **any** results.
Uses a local Ollama model (llama3.1) as a relevance judge to score retrieved
facts on a 1–3 scale, then computes precision, recall, and MRR metrics.

This is distinct from the `e2e/` functional test suite, which checks that
commands succeed and outputs are well-formed. This eval checks that the
*right* facts are returned and ranked correctly.

## Prerequisites

- Engram binary built (`cargo build --release`)
- [Ollama](https://ollama.ai) installed with the llama3.1 model:
  ```bash
  ollama pull llama3.1
  ollama serve   # if not already running
  ```

## Running

```bash
cd eval
python3 run_eval.py
```

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `ENGRAM_BIN` | `../target/release/engram` | Path to the engram binary |
| `OLLAMA_BASE_URL` | `http://localhost:11434` | Ollama API endpoint |
| `OLLAMA_MODEL` | `llama3.1` | Model used for relevance judging |

## Interpreting the Scorecard

The eval prints a summary scorecard after running all 25 queries:

| Metric | What It Measures | Ideal |
|--------|-----------------|-------|
| **Precision@3** | Of the top 3 results, what fraction are relevant (Ollama score >= 2)? | 1.00 |
| **Recall@5** | Of the expected facts (`must_include`), what fraction appear in top 5? | 1.00 |
| **MRR** | Mean Reciprocal Rank — how early does the first expected fact appear? 1/rank. | 1.00 |
| **Exclusion violations** | How many facts that should NOT appear (e.g. expired) showed up in top 5? | 0 |

Metrics are reported overall and broken down by query tier (BM25, temporal,
causal, mixed).

## Corpus

The `corpus/` directory contains 30 fact files across three domains:

- **Oil & Gas (12):** Well integrity, pressure testing, regulatory, HSE
- **Infrastructure (10):** Kubernetes, Docker, networking, database, observability
- **Software Engineering (8):** Rust, PostgreSQL, API design, security

This is larger than the `e2e/` corpus (10 facts) to create real competition
between facts for ranking.

## Query Set

`queries.json` contains 25 static queries with expected results:

- **BM25 (8):** Direct keyword queries — test basic retrieval precision
- **Temporal (5):** Queries with time signal words — test state/event handling
- **Causal (6):** Queries with causal signal words — test graph traversal
- **Mixed (6):** Ambiguous queries that match multiple facts — test ranking

**`queries.json` is committed and static.** Do not regenerate it unless the
corpus changes. It defines the benchmark.

## Results

Each run saves a timestamped JSON file in `results/` (gitignored) containing
per-query metrics and Ollama relevance scores. Multiple runs accumulate for
trend tracking.

```
eval/results/
  20240315-143022.json
  20240316-091545.json
  ...
```
