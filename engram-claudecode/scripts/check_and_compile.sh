#!/usr/bin/env bash
set -euo pipefail

# check_and_compile.sh — Locate workspace, check dirty flag, compile if needed.
# Prints the resolved workspace path to stdout if valid.
# Prints nothing if no workspace or no binary found.

# 1. Locate workspace
if [ -n "${ENGRAM_WORKSPACE:-}" ] && [ -d "$ENGRAM_WORKSPACE/.brv" ]; then
    WORKSPACE="$ENGRAM_WORKSPACE"
elif [ -n "${CLAUDE_PROJECT_DIR:-}" ] && [ -d "$CLAUDE_PROJECT_DIR/.brv" ]; then
    WORKSPACE="$CLAUDE_PROJECT_DIR"
else
    # No .brv/ found — auto-initialize if we have a project dir and binary
    if [ -z "${CLAUDE_PROJECT_DIR:-}" ]; then
        exit 0
    fi

    # Resolve binary before attempting init
    if [ -n "${ENGRAM_BIN:-}" ] && [ -x "$ENGRAM_BIN" ]; then
        _BIN="$ENGRAM_BIN"
    elif command -v engram &>/dev/null; then
        _BIN="$(command -v engram)"
    else
        exit 0
    fi

    # Initialize the workspace silently
    (cd "$CLAUDE_PROJECT_DIR" && "$_BIN" init) >/dev/null 2>&1 || true

    if [ -d "$CLAUDE_PROJECT_DIR/.brv" ]; then
        WORKSPACE="$CLAUDE_PROJECT_DIR"
    else
        exit 0
    fi
fi

# 2. Locate binary
if [ -n "${ENGRAM_BIN:-}" ] && [ -x "$ENGRAM_BIN" ]; then
    BIN="$ENGRAM_BIN"
elif command -v engram &>/dev/null; then
    BIN="$(command -v engram)"
elif [ -x "$WORKSPACE/target/release/engram" ]; then
    BIN="$WORKSPACE/target/release/engram"
else
    echo "WARN: engram binary not found, skipping compile" >&2
    exit 0
fi

# 3. Check dirty flag
STATE_FILE="$WORKSPACE/.brv/index/state"
NEEDS_COMPILE=false

if [ ! -f "$STATE_FILE" ]; then
    # No state file — index not yet built, needs full compile
    NEEDS_COMPILE=true
else
    # Parse dirty field from JSON (simple grep — avoids jq dependency)
    if grep -q '"dirty": *true' "$STATE_FILE" 2>/dev/null; then
        NEEDS_COMPILE=true
    fi
fi

# 4. Compile if needed
if [ "$NEEDS_COMPILE" = true ]; then
    if ! (cd "$WORKSPACE" && "$BIN" compile --incremental) >/dev/null 2>&1; then
        echo "WARN: engram compile failed, querying stale index" >&2
    fi
fi

# 5. Output workspace path
echo "$WORKSPACE"
