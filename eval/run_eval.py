#!/usr/bin/env python3
"""
Engram Retrieval Quality Evaluation

Measures whether Engram returns *relevant* results using a local Ollama
model as a relevance judge. Runs 25 queries against a 30-fact corpus
and produces a scorecard with Precision@3, Recall@5, and MRR metrics.

Usage:
    cd eval && python3 run_eval.py

Environment variables:
    ENGRAM_BIN      Path to the engram binary (default: ../target/release/engram)
    OLLAMA_BASE_URL Ollama API URL (default: http://localhost:11434)
    OLLAMA_MODEL    Model to use for judging (default: llama3.1)
"""

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from datetime import datetime, timezone

# Try requests first, fall back to urllib
try:
    import requests
    HAS_REQUESTS = True
except ImportError:
    import urllib.request
    import urllib.error
    HAS_REQUESTS = False

# ── Configuration ────────────────────────────────────────────────────

SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
ENGRAM_BIN = os.environ.get("ENGRAM_BIN", os.path.join(SCRIPT_DIR, "..", "target", "release", "engram"))
CORPUS_DIR = os.path.join(SCRIPT_DIR, "corpus")
QUERIES_FILE = os.path.join(SCRIPT_DIR, "queries.json")
RESULTS_DIR = os.path.join(SCRIPT_DIR, "results")
OLLAMA_BASE_URL = os.environ.get("OLLAMA_BASE_URL", "http://localhost:11434")
OLLAMA_MODEL = os.environ.get("OLLAMA_MODEL", "llama3.1")
TOP_N = 5


# ── HTTP helper ──────────────────────────────────────────────────────

def http_post_json(url, payload, timeout=60):
    """POST JSON and return parsed response. Works with or without requests."""
    if HAS_REQUESTS:
        resp = requests.post(url, json=payload, timeout=timeout)
        resp.raise_for_status()
        return resp.json()
    else:
        data = json.dumps(payload).encode("utf-8")
        req = urllib.request.Request(
            url,
            data=data,
            headers={"Content-Type": "application/json"},
        )
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return json.loads(resp.read().decode("utf-8"))


# ── Workspace Setup ──────────────────────────────────────────────────

def setup_workspace(corpus_dir, engram_bin):
    """
    Creates a temp workspace, copies corpus into .brv/context-tree/,
    writes engram.toml with score_threshold = 0.0, runs engram compile.
    Returns workspace path.
    """
    ws = tempfile.mkdtemp(prefix="engram_eval_")
    ctx_dir = os.path.join(ws, ".brv", "context-tree")
    os.makedirs(ctx_dir)

    # Copy corpus
    for f in os.listdir(corpus_dir):
        if f.endswith(".md"):
            shutil.copy2(os.path.join(corpus_dir, f), ctx_dir)

    # Write config with low threshold
    config_path = os.path.join(ws, ".brv", "engram.toml")
    with open(config_path, "w") as fh:
        fh.write("[query]\nscore_threshold = 0.0\n")

    # Compile
    result = subprocess.run(
        [engram_bin, "compile"],
        cwd=ws,
        capture_output=True,
        text=True,
    )

    if result.returncode != 0:
        print(f"ERROR: engram compile failed in {ws}", file=sys.stderr)
        print(f"stdout: {result.stdout}", file=sys.stderr)
        print(f"stderr: {result.stderr}", file=sys.stderr)
        shutil.rmtree(ws, ignore_errors=True)
        sys.exit(1)

    return ws


# ── Query Runner ─────────────────────────────────────────────────────

def run_query(engram_bin, workspace, query):
    """
    Runs: engram query "<query>" in the workspace.
    Parses stdout to extract results.
    Returns list of dicts: [{id, title, score, source_path}, ...]
    """
    result = subprocess.run(
        [engram_bin, "query", query],
        cwd=workspace,
        capture_output=True,
        text=True,
    )

    hits = []
    for line in result.stdout.splitlines():
        # Match lines like: "  1. [score: 0.234] Title (source_path)"
        m = re.match(
            r'\s*\d+\.\s+\[score:\s+([\d.]+)\]\s+(.*?)\s+\((.+)\)\s*$',
            line,
        )
        if m:
            score = float(m.group(1))
            title = m.group(2)
            source_path = m.group(3)
            # Derive fact ID from source path (same as Engram: strip prefix + .md)
            fact_id = os.path.basename(source_path)
            if fact_id.endswith(".md"):
                fact_id = fact_id[:-3]
            hits.append({
                "id": fact_id,
                "title": title,
                "score": score,
                "source_path": source_path,
            })

    return hits


# ── Fact Body Loading ────────────────────────────────────────────────

def strip_frontmatter(content):
    """Strip YAML frontmatter delimited by ---."""
    if not content.startswith("---"):
        return content
    # Find closing ---
    rest = content[3:]
    if rest.startswith("\n"):
        rest = rest[1:]
    idx = rest.find("\n---")
    if idx == -1:
        return content
    after = rest[idx + 4:]
    if after.startswith("\n"):
        after = after[1:]
    return after


def load_fact_bodies(corpus_dir, fact_ids):
    """Load body text for given fact IDs. Returns list of {id, title, body}."""
    bodies = []
    for fid in fact_ids:
        path = os.path.join(corpus_dir, f"{fid}.md")
        if not os.path.exists(path):
            continue
        with open(path, "r") as f:
            content = f.read()
        body = strip_frontmatter(content).strip()
        # Truncate to 300 chars
        if len(body) > 300:
            body = body[:300] + "..."

        # Extract title from frontmatter
        title = fid
        title_match = re.search(r'^title:\s*["\']?(.*?)["\']?\s*$', content, re.MULTILINE)
        if title_match:
            title = title_match.group(1)

        bodies.append({"id": fid, "title": title, "body": body})
    return bodies


# ── Ollama Judge ─────────────────────────────────────────────────────

def judge_relevance(query, fact_bodies, ollama_base_url, model):
    """
    Sends query + retrieved fact bodies to Ollama for relevance scoring.
    Returns dict mapping fact_id -> relevance score (1, 2, or 3).
    """
    if not fact_bodies:
        return {}

    facts_text = ""
    for fb in fact_bodies:
        facts_text += f"ID: {fb['id']}\nTitle: {fb['title']}\nContent: {fb['body']}\n---\n"

    prompt = f"""You are evaluating a retrieval system. Given a query and retrieved facts, rate each fact's relevance to the query.

Query: {query}

Facts:
{facts_text}

Respond with ONLY a JSON object mapping each fact ID to a relevance score:
- 3 = highly relevant (directly answers the query)
- 2 = somewhat relevant (related but not the best answer)
- 1 = not relevant (off-topic)

Example response format:
{{"fact_id_1": 3, "fact_id_2": 1, "fact_id_3": 2}}

Respond with JSON only. No explanation."""

    try:
        resp = http_post_json(
            f"{ollama_base_url}/api/generate",
            {
                "model": model,
                "prompt": prompt,
                "stream": False,
                "format": "json",
            },
            timeout=60,
        )

        raw_response = resp.get("response", "")

        # Strip markdown fences if present
        cleaned = raw_response.strip()
        cleaned = re.sub(r'^```(?:json)?\s*', '', cleaned)
        cleaned = re.sub(r'\s*```$', '', cleaned)
        cleaned = cleaned.strip()

        scores = json.loads(cleaned)

        # Validate and normalize scores
        result = {}
        for fb in fact_bodies:
            fid = fb["id"]
            score = scores.get(fid, 1)
            if isinstance(score, (int, float)):
                score = max(1, min(3, int(score)))
            else:
                score = 1
            result[fid] = score
        return result

    except (json.JSONDecodeError, KeyError, TypeError) as e:
        print(f"  WARN: Ollama JSON parse error: {e}", file=sys.stderr)
        print(f"  Raw response: {raw_response[:200]}", file=sys.stderr)
        return {fb["id"]: 1 for fb in fact_bodies}
    except Exception as e:
        print(f"  WARN: Ollama call failed: {e}", file=sys.stderr)
        return {fb["id"]: 1 for fb in fact_bodies}


# ── Metrics Calculation ──────────────────────────────────────────────

def calculate_metrics(query_spec, retrieved_ids, ollama_scores):
    """
    Returns:
      precision_at_3: float   - of top 3, fraction with ollama score >= 2
      recall_at_5: float      - of must_include facts, fraction in top 5
      mrr: float              - 1/rank of first must_include fact
      must_exclude_violations: int - count of must_exclude facts in top 5
      tier_hint: str          - from query spec
    """
    # Precision@3: of top 3, fraction judged relevant (score >= 2)
    top3 = retrieved_ids[:3]
    if top3:
        relevant_count = sum(1 for fid in top3 if ollama_scores.get(fid, 1) >= 2)
        precision_at_3 = relevant_count / len(top3)
    else:
        precision_at_3 = 0.0

    # Recall@5: of must_include, fraction in top 5
    must_include = query_spec.get("must_include", [])
    top5 = retrieved_ids[:5]
    if must_include:
        found = sum(1 for fid in must_include if fid in top5)
        recall_at_5 = found / len(must_include)
    else:
        recall_at_5 = 1.0

    # MRR: 1/rank of first must_include fact
    if must_include:
        mrr = 0.0
        for i, fid in enumerate(retrieved_ids[:5]):
            if fid in must_include:
                mrr = 1.0 / (i + 1)
                break
    else:
        mrr = 1.0

    # Must-exclude violations
    must_exclude = query_spec.get("must_exclude", [])
    violations = sum(1 for fid in must_exclude if fid in top5)

    return {
        "precision_at_3": precision_at_3,
        "recall_at_5": recall_at_5,
        "mrr": mrr,
        "must_exclude_violations": violations,
        "tier_hint": query_spec.get("tier_hint", "unknown"),
    }


# ── Main Eval Loop ───────────────────────────────────────────────────

def run_eval(queries, engram_bin, workspace, corpus_dir):
    results = []
    for q in queries:
        retrieved = run_query(engram_bin, workspace, q["query"])
        retrieved_ids = [r["id"] for r in retrieved[:TOP_N]]

        # Get bodies for Ollama
        fact_bodies = load_fact_bodies(corpus_dir, retrieved_ids)

        # Judge relevance
        ollama_scores = judge_relevance(
            q["query"], fact_bodies, OLLAMA_BASE_URL, OLLAMA_MODEL
        )

        # Calculate metrics
        metrics = calculate_metrics(q, retrieved_ids, ollama_scores)
        results.append({
            "query_id": q["id"],
            "query": q["query"],
            "retrieved_ids": retrieved_ids,
            "ollama_scores": ollama_scores,
            **metrics,
        })

        # Print per-query result
        print(f"  {q['id']} [{q['tier_hint']:8}] "
              f"P@3={metrics['precision_at_3']:.2f} "
              f"R@5={metrics['recall_at_5']:.2f} "
              f"MRR={metrics['mrr']:.2f} "
              f"excl={metrics['must_exclude_violations']} "
              f"| {q['query'][:50]}")

    return results


def print_scorecard(results, model, corpus_size, query_count):
    """Print and return the summary scorecard."""
    # Overall averages
    avg_p3 = sum(r["precision_at_3"] for r in results) / len(results)
    avg_r5 = sum(r["recall_at_5"] for r in results) / len(results)
    avg_mrr = sum(r["mrr"] for r in results) / len(results)
    total_violations = sum(r["must_exclude_violations"] for r in results)

    # By tier
    tiers = {}
    for r in results:
        t = r["tier_hint"]
        if t not in tiers:
            tiers[t] = []
        tiers[t].append(r)

    tier_stats = {}
    for t, tier_results in sorted(tiers.items()):
        tier_stats[t] = {
            "count": len(tier_results),
            "precision_at_3": sum(r["precision_at_3"] for r in tier_results) / len(tier_results),
            "recall_at_5": sum(r["recall_at_5"] for r in tier_results) / len(tier_results),
            "mrr": sum(r["mrr"] for r in tier_results) / len(tier_results),
        }

    # Worst queries
    worst = [r for r in results if r["mrr"] == 0.0 and r.get("must_include_count", len([x for x in [r] if r.get("query_id")]))]
    # Filter to only those with must_include expectations
    worst = [r for r in results if r["mrr"] == 0.0]

    print()
    print("=" * 55)
    print(f"  Engram Retrieval Quality Scorecard")
    print(f"  Model: {model}   Corpus: {corpus_size} facts   Queries: {query_count}")
    print("=" * 55)
    print()
    print("  Overall")
    print("  " + "-" * 47)
    print(f"  Precision@3 (avg):        {avg_p3:.2f}")
    print(f"  Recall@5    (avg):        {avg_r5:.2f}")
    print(f"  MRR         (avg):        {avg_mrr:.2f}")
    print(f"  Exclusion violations:     {total_violations}")
    print()
    print("  By Tier")
    print("  " + "-" * 47)
    for t, s in sorted(tier_stats.items()):
        print(f"  {t:8} ({s['count']}q):  "
              f"P@3={s['precision_at_3']:.2f}  "
              f"R@5={s['recall_at_5']:.2f}  "
              f"MRR={s['mrr']:.2f}")

    if worst:
        print()
        print("  Worst Queries (MRR = 0)")
        print("  " + "-" * 47)
        for r in worst:
            print(f"  {r['query_id']}: {r['query'][:50]}")

    print()
    print("=" * 55)

    return {
        "precision_at_3": avg_p3,
        "recall_at_5": avg_r5,
        "mrr": avg_mrr,
        "exclusion_violations": total_violations,
        "by_tier": tier_stats,
    }


def save_results(results, summary, model, corpus_size, query_count):
    """Save full results to eval/results/YYYYMMDD-HHMMSS.json."""
    os.makedirs(RESULTS_DIR, exist_ok=True)
    timestamp = datetime.now(timezone.utc).strftime("%Y%m%d-%H%M%S")
    output = {
        "timestamp": datetime.now(timezone.utc).isoformat(),
        "model": model,
        "corpus_size": corpus_size,
        "query_count": query_count,
        "summary": summary,
        "per_query": results,
    }
    path = os.path.join(RESULTS_DIR, f"{timestamp}.json")
    with open(path, "w") as f:
        json.dump(output, f, indent=2)
    print(f"\nResults saved to: {path}")


# ── Entry Point ──────────────────────────────────────────────────────

def main():
    # Resolve engram binary
    engram_bin = os.path.abspath(ENGRAM_BIN)
    if not os.path.isfile(engram_bin) or not os.access(engram_bin, os.X_OK):
        print(f"ERROR: engram binary not found at {engram_bin}", file=sys.stderr)
        print("Build with: cargo build --release", file=sys.stderr)
        sys.exit(1)

    # Load queries
    with open(QUERIES_FILE, "r") as f:
        queries = json.load(f)

    corpus_size = len([f for f in os.listdir(CORPUS_DIR) if f.endswith(".md")])

    print(f"Engram Retrieval Quality Eval")
    print(f"  Binary:  {engram_bin}")
    print(f"  Corpus:  {corpus_size} facts")
    print(f"  Queries: {len(queries)}")
    print(f"  Model:   {OLLAMA_MODEL}")
    print(f"  Ollama:  {OLLAMA_BASE_URL}")
    print()

    # Check Ollama is reachable
    try:
        http_post_json(
            f"{OLLAMA_BASE_URL}/api/generate",
            {"model": OLLAMA_MODEL, "prompt": "test", "stream": False},
            timeout=30,
        )
    except Exception as e:
        print(f"ERROR: Cannot reach Ollama at {OLLAMA_BASE_URL}: {e}", file=sys.stderr)
        print("Start Ollama with: ollama serve", file=sys.stderr)
        sys.exit(1)

    # Setup workspace
    print("Setting up workspace...")
    workspace = setup_workspace(CORPUS_DIR, engram_bin)
    print(f"  Workspace: {workspace}")
    print()

    # Run eval
    print("Running queries...")
    try:
        results = run_eval(queries, engram_bin, workspace, CORPUS_DIR)
        summary = print_scorecard(results, OLLAMA_MODEL, corpus_size, len(queries))
        save_results(results, summary, OLLAMA_MODEL, corpus_size, len(queries))
    finally:
        # Cleanup workspace
        shutil.rmtree(workspace, ignore_errors=True)


if __name__ == "__main__":
    main()
