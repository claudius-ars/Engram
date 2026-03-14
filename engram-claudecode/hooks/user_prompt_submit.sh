#!/usr/bin/env bash
set -euo pipefail

# user_prompt_submit.sh — UserPromptSubmit hook for Claude Code.
# Reads the user's prompt from stdin, queries Engram, formats results,
# and writes a context block to stdout for injection into the conversation.
#
# Exit 0 in ALL cases — a non-zero exit aborts the Claude Code session.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PLUGIN_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Read prompt from stdin
PROMPT="$(cat)"

# Skip very short prompts (typos, single words, slash commands)
if [ "${#PROMPT}" -lt 10 ]; then
    exit 0
fi

# Check workspace and compile if dirty
WORKSPACE="$("$PLUGIN_ROOT/scripts/check_and_compile.sh" 2>/dev/null)" || true

# If no workspace found, exit silently
if [ -z "$WORKSPACE" ]; then
    exit 0
fi

# Locate binary (check_and_compile.sh already verified it exists,
# but re-resolve here for the query call)
if [ -n "${ENGRAM_BIN:-}" ] && [ -x "$ENGRAM_BIN" ]; then
    BIN="$ENGRAM_BIN"
elif command -v engram &>/dev/null; then
    BIN="$(command -v engram)"
elif [ -x "$WORKSPACE/target/release/engram" ]; then
    BIN="$WORKSPACE/target/release/engram"
else
    exit 0
fi

# Agent ID for Bulwark policy
AGENT_ID="${ENGRAM_AGENT_ID:-claude-code}"

# Run query with JSON format for body content
RESULTS="$(
    cd "$WORKSPACE" && \
    "$BIN" query \
        --format json \
        --agent "$AGENT_ID" \
        "$PROMPT" \
    2>/dev/null
)" || true

# Format and inject
if [ -n "$RESULTS" ]; then
    echo "$RESULTS" | "$PLUGIN_ROOT/scripts/format_results.sh"
fi
