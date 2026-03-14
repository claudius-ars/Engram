#!/usr/bin/env bash
set -euo pipefail

# session_end.sh — Stop hook for Claude Code.
# Prints a curation reminder when an Engram workspace exists.
# Exit 0 in ALL cases — a non-zero exit aborts the session.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Locate workspace
WORKSPACE=""
if [ -n "${ENGRAM_WORKSPACE:-}" ] && [ -d "$ENGRAM_WORKSPACE/.brv" ]; then
    WORKSPACE="$ENGRAM_WORKSPACE"
elif [ -n "${CLAUDE_PROJECT_DIR:-}" ] && \
     [ -d "$CLAUDE_PROJECT_DIR/.brv" ]; then
    WORKSPACE="$CLAUDE_PROJECT_DIR"
fi

# Only emit reminder if a workspace exists
if [ -z "$WORKSPACE" ]; then
    exit 0
fi

# Emit a curation prompt to stdout
cat << 'EOF'
---
**Engram:** Before ending — did this session produce any facts worth
remembering for future sessions? Decisions made, constraints discovered,
or state that changed?

If yes, use the memory-recall skill or run:
`engram curate --sync "your fact here"`
---
EOF
