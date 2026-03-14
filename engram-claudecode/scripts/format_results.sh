#!/usr/bin/env bash
set -euo pipefail

# format_results.sh — Transform engram query output into a context block.
# Reads engram CLI output from stdin (NDJSON or text), writes formatted
# block to stdout. Writes nothing if no results found.
#
# Primary format: NDJSON (--format json) — includes body text.
# Fallback: text format — title and fact ID only (no body).

MAX_RESULTS="${ENGRAM_MAX_RESULTS:-3}"

# Read all input
INPUT="$(cat)"

# Exit silently if empty
if [ -z "$INPUT" ]; then
    exit 0
fi

# Detect format: NDJSON lines start with `{`
FIRST_CHAR="$(echo "$INPUT" | head -c1)"

if [ "$FIRST_CHAR" = "{" ] && command -v python3 &>/dev/null; then
    # NDJSON path — use python3 for reliable JSON parsing
    echo "$INPUT" | python3 -c "
import sys, json

max_results = int(sys.argv[1])
results = []
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    try:
        results.append(json.loads(line))
    except json.JSONDecodeError:
        continue

results = results[:max_results]
if not results:
    sys.exit(0)

print('---')
print('## Engram Memory Context')
print('The following facts from the project knowledge base are relevant')
print('to your current task:')
print()
for r in results:
    title = r.get('title') or r.get('fact_id', 'Unknown')
    fact_id = r.get('fact_id', '')
    body = r.get('body', '')
    label = f' \`[{fact_id}]\`' if fact_id else ''
    print(f'**{title}**{label}')
    if body:
        print(body)
    print()
print('*Retrieved by Engram — project memory layer*')
print('---')
" "$MAX_RESULTS"
    exit 0
fi

# Text format fallback: parse "N. [score: X.XXX] Title (source_path)"
if echo "$INPUT" | grep -q "No results found"; then
    exit 0
fi

RESULT_LINES="$(echo "$INPUT" | grep -E '^ +[0-9]+\. \[score:' || true)"

if [ -z "$RESULT_LINES" ]; then
    exit 0
fi

COUNT=0
TITLES=()
FACT_IDS=()

while IFS= read -r line; do
    COUNT=$((COUNT + 1))
    if [ "$COUNT" -gt "$MAX_RESULTS" ]; then
        break
    fi

    title="$(echo "$line" | sed -E 's/^ +[0-9]+\. \[score: [0-9.]+\] //' | sed -E 's/ \([^)]*\)$//')"
    source_path="$(echo "$line" | grep -oE '\([^)]+\)$' | tr -d '()')"
    fact_id=""
    if [ -n "$source_path" ]; then
        fact_id="$(basename "$source_path" .md)"
    fi

    TITLES+=("$title")
    FACT_IDS+=("$fact_id")
done <<< "$RESULT_LINES"

if [ "${#TITLES[@]}" -eq 0 ]; then
    exit 0
fi

echo "---"
echo "## Engram Memory Context"
echo "The following facts from the project knowledge base are relevant"
echo "to your current task:"
echo ""

for i in "${!TITLES[@]}"; do
    title="${TITLES[$i]}"
    fact_id="${FACT_IDS[$i]}"

    if [ -n "$fact_id" ]; then
        echo "**${title}** \`[${fact_id}]\`"
    else
        echo "**${title}**"
    fi
    echo ""
done

echo "*Retrieved by Engram — project memory layer*"
echo "---"
