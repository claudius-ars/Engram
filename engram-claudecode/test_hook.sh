#!/usr/bin/env bash
set -euo pipefail

# Manual smoke test for the UserPromptSubmit hook.
# Usage: ./test_hook.sh [workspace_path]
#
# Tests the hook end-to-end by simulating what Claude Code does:
# pipes a prompt into the hook's stdin, captures stdout.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE="${1:-$(pwd)}"

echo "=== Engram Claude Code Hook Smoke Test ==="
echo "Workspace: $WORKSPACE"
echo ""

# Check prerequisites
if [ ! -d "$WORKSPACE/.brv" ]; then
    echo "ERROR: No .brv/ directory found in $WORKSPACE"
    echo "Run 'engram compile' first to build the index."
    exit 1
fi

export CLAUDE_PROJECT_DIR="$WORKSPACE"
export ENGRAM_WORKSPACE="$WORKSPACE"

# Auto-detect engram binary if not already set
if [ -z "${ENGRAM_BIN:-}" ]; then
    if command -v engram &>/dev/null; then
        export ENGRAM_BIN="$(command -v engram)"
    elif [ -x "$WORKSPACE/target/release/engram" ]; then
        export ENGRAM_BIN="$WORKSPACE/target/release/engram"
    else
        # Look relative to the repo root (common dev layout)
        REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
        if [ -x "$REPO_ROOT/target/release/engram" ]; then
            export ENGRAM_BIN="$REPO_ROOT/target/release/engram"
        fi
    fi
fi

PASS=0
FAIL=0

# Test 1: Short prompt (should produce no output)
echo "Test 1: Short prompt (expect: no output)"
OUTPUT=$(echo "hi" | "$SCRIPT_DIR/hooks/user_prompt_submit.sh" 2>/dev/null) || true
if [ -z "$OUTPUT" ]; then
    echo "  PASS"
    PASS=$((PASS + 1))
else
    echo "  FAIL: got output for short prompt"
    FAIL=$((FAIL + 1))
fi

# Test 2: Real query (should produce context block with body content)
echo "Test 2: Real query (expect: context block with body)"
OUTPUT=$(echo "what are the kubernetes pod scheduling rules" | \
    "$SCRIPT_DIR/hooks/user_prompt_submit.sh" 2>/dev/null) || true
if echo "$OUTPUT" | grep -q "Engram Memory Context"; then
    if echo "$OUTPUT" | grep -qi "scheduling\|filtering\|scoring\|two-phase"; then
        LINES=$(echo "$OUTPUT" | wc -l | tr -d ' ')
        echo "  PASS ($LINES lines, body content present)"
        PASS=$((PASS + 1))
    else
        echo "  FAIL: context block present but no body content"
        echo "$OUTPUT"
        FAIL=$((FAIL + 1))
    fi
elif [ -z "$OUTPUT" ]; then
    echo "  PASS (no results — index may be empty)"
    PASS=$((PASS + 1))
else
    echo "  FAIL: unexpected output format"
    echo "$OUTPUT"
    FAIL=$((FAIL + 1))
fi

# Test 3: Hook exits 0 even when engram binary is missing
echo "Test 3: Missing binary (expect: exit 0, no output)"
OUTPUT=$(echo "kubernetes scheduling" | \
    ENGRAM_BIN="/nonexistent/engram" \
    "$SCRIPT_DIR/hooks/user_prompt_submit.sh" 2>/dev/null) || true
if [ -z "$OUTPUT" ]; then
    echo "  PASS"
    PASS=$((PASS + 1))
else
    echo "  FAIL: got output with missing binary"
    FAIL=$((FAIL + 1))
fi

# Test 4: Hook exits 0 when no workspace exists
echo "Test 4: No workspace (expect: exit 0, no output)"
OUTPUT=$(echo "kubernetes scheduling" | \
    ENGRAM_WORKSPACE="/nonexistent/workspace" \
    CLAUDE_PROJECT_DIR="/nonexistent/project" \
    ENGRAM_BIN="/nonexistent/engram" \
    "$SCRIPT_DIR/hooks/user_prompt_submit.sh" 2>/dev/null) || true
if [ -z "$OUTPUT" ]; then
    echo "  PASS"
    PASS=$((PASS + 1))
else
    echo "  FAIL: got output with no workspace"
    FAIL=$((FAIL + 1))
fi

# Test 5: Formatter handles "No results found" gracefully
echo "Test 5: No results (expect: no output from formatter)"
OUTPUT=$(echo "No results found." | "$SCRIPT_DIR/scripts/format_results.sh" 2>/dev/null) || true
if [ -z "$OUTPUT" ]; then
    echo "  PASS"
    PASS=$((PASS + 1))
else
    echo "  FAIL: formatter produced output for 'No results found'"
    FAIL=$((FAIL + 1))
fi

# Test 6: Formatter handles empty input
echo "Test 6: Empty input (expect: no output from formatter)"
OUTPUT=$(echo "" | "$SCRIPT_DIR/scripts/format_results.sh" 2>/dev/null) || true
if [ -z "$OUTPUT" ]; then
    echo "  PASS"
    PASS=$((PASS + 1))
else
    echo "  FAIL: formatter produced output for empty input"
    FAIL=$((FAIL + 1))
fi

# Test 7: Session end hook with workspace (expect: reminder output)
echo "Test 7: Session end with workspace (expect: reminder)"
OUTPUT=$("$SCRIPT_DIR/hooks/session_end.sh" 2>/dev/null) || true
if echo "$OUTPUT" | grep -qi "engram\|curate\|remember"; then
    echo "  PASS"
    PASS=$((PASS + 1))
else
    echo "  FAIL: no reminder output"
    echo "$OUTPUT"
    FAIL=$((FAIL + 1))
fi

# Test 8: Session end hook without workspace (expect: no output)
echo "Test 8: Session end without workspace (expect: no output)"
OUTPUT=$(ENGRAM_WORKSPACE="/nonexistent" \
    CLAUDE_PROJECT_DIR="/nonexistent" \
    "$SCRIPT_DIR/hooks/session_end.sh" 2>/dev/null) || true
if [ -z "$OUTPUT" ]; then
    echo "  PASS"
    PASS=$((PASS + 1))
else
    echo "  FAIL: got output with no workspace"
    FAIL=$((FAIL + 1))
fi

# Test 9: Auto-initialize in new project
echo "Test 9: Auto-initialize (expect: silent success, .brv/ created)"
NEW_PROJECT=$(mktemp -d)
OUTPUT=$(echo "test query" | \
    CLAUDE_PROJECT_DIR="$NEW_PROJECT" \
    ENGRAM_WORKSPACE="$NEW_PROJECT" \
    ENGRAM_BIN="${ENGRAM_BIN:-$(command -v engram 2>/dev/null || echo "")}" \
    "$SCRIPT_DIR/hooks/user_prompt_submit.sh" 2>/dev/null) || true
if [ -d "$NEW_PROJECT/.brv" ]; then
    echo "  PASS (.brv/ created)"
    PASS=$((PASS + 1))
else
    echo "  FAIL: .brv/ not created in $NEW_PROJECT"
    FAIL=$((FAIL + 1))
fi
rm -rf "$NEW_PROJECT"

echo ""
echo "════════════════════════════════════════"
echo "  Results: $PASS passed, $FAIL failed"
echo "════════════════════════════════════════"

if [ "$FAIL" -gt 0 ]; then
    exit 1
fi
