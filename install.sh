#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PLUGIN_ROOT="$REPO_ROOT/engram-claudecode"
SKILL_PATH="$PLUGIN_ROOT/skill/SKILL.md"

# ── 1. Check prerequisites ──────────────────────────────────────────

if ! command -v cargo &>/dev/null; then
    echo "✗ cargo not found. Install Rust from https://rustup.rs"
    exit 1
fi

if ! command -v claude &>/dev/null; then
    echo "✗ claude not found. Install Claude Code first."
    echo "  https://code.claude.com"
    exit 1
fi

if ! command -v python3 &>/dev/null; then
    echo "✗ python3 not found. Required for settings file merging."
    exit 1
fi

# ── 2. Build the binary ─────────────────────────────────────────────

echo "→ Building Engram..."
cargo build --release -p engram-cli -q
echo "✓ Build complete"

# ── 3. Install the binary ───────────────────────────────────────────

INSTALL_DIR=""
if [ -d "$HOME/.local/bin" ] && echo "$PATH" | grep -q "$HOME/.local/bin"; then
    INSTALL_DIR="$HOME/.local/bin"
elif [ -d "/usr/local/bin" ]; then
    INSTALL_DIR="/usr/local/bin"
else
    mkdir -p "$HOME/.local/bin"
    INSTALL_DIR="$HOME/.local/bin"
    echo "  Note: add $HOME/.local/bin to your PATH"
fi

cp "$REPO_ROOT/target/release/engram" "$INSTALL_DIR/engram"
echo "✓ Binary installed to $INSTALL_DIR/engram"

# ── 4. Locate Claude Code user settings ─────────────────────────────

if [[ "$OSTYPE" == "darwin"* ]]; then
    CLAUDE_SETTINGS="$HOME/.claude/settings.json"
else
    CLAUDE_SETTINGS="$HOME/.claude/settings.json"
fi

mkdir -p "$(dirname "$CLAUDE_SETTINGS")"

# ── 5. Merge hooks and skill into settings ───────────────────────────

python3 - "$CLAUDE_SETTINGS" "$PLUGIN_ROOT" "$SKILL_PATH" << 'PYEOF'
import json, os, sys, shutil

settings_path = sys.argv[1]
plugin_root   = sys.argv[2]
skill_path    = sys.argv[3]

new_hooks = {
    "UserPromptSubmit": [
        {
            "matcher": ".*",
            "hooks": [
                {
                    "type": "command",
                    "command": f"{plugin_root}/hooks/user_prompt_submit.sh"
                }
            ]
        }
    ],
    "Stop": [
        {
            "hooks": [
                {
                    "type": "command",
                    "command": f"{plugin_root}/hooks/session_end.sh"
                }
            ]
        }
    ]
}

# Load existing settings or start fresh
if os.path.exists(settings_path) and os.path.getsize(settings_path) > 0:
    with open(settings_path) as f:
        try:
            settings = json.load(f)
        except json.JSONDecodeError:
            print(f"  Warning: backing up unparseable {settings_path}")
            shutil.copy(settings_path, settings_path + ".bak")
            settings = {}
else:
    settings = {}

# Merge hooks — preserve existing hooks, avoid duplicates
existing_hooks = settings.get("hooks", {})
for event, entries in new_hooks.items():
    if event not in existing_hooks:
        existing_hooks[event] = entries
    else:
        existing_commands = [
            h.get("command", "")
            for block in existing_hooks[event]
            for h in block.get("hooks", [])
        ]
        engram_cmd = entries[0]["hooks"][0]["command"]
        if any("engram-claudecode" in cmd for cmd in existing_commands):
            # Update existing Engram hook path
            for block in existing_hooks[event]:
                for h in block.get("hooks", []):
                    if "engram-claudecode" in h.get("command", ""):
                        h["command"] = engram_cmd
        else:
            existing_hooks[event].extend(entries)

settings["hooks"] = existing_hooks

# Register skill directory via additionalDirectories.
# Claude Code discovers skills from .claude/skills/<name>/SKILL.md
# within directories listed in permissions.additionalDirectories.
# The plugin root contains .claude/skills/memory-recall/SKILL.md
# (symlinked to skill/SKILL.md) for this discovery mechanism.
permissions = settings.get("permissions", {})
additional_dirs = permissions.get("additionalDirectories", [])
if plugin_root not in additional_dirs:
    additional_dirs.append(plugin_root)
    permissions["additionalDirectories"] = additional_dirs
    settings["permissions"] = permissions

with open(settings_path, "w") as f:
    json.dump(settings, f, indent=2)
    f.write("\n")
PYEOF

echo "✓ Hooks and skill registered in $CLAUDE_SETTINGS"

# ── 6. Summary ───────────────────────────────────────────────────────

echo ""
echo "═══════════════════════════════════════"
echo "  Engram installed successfully"
echo "═══════════════════════════════════════"
echo ""
echo "  Binary:   $INSTALL_DIR/engram"
echo "  Plugin:   $PLUGIN_ROOT"
echo "  Skill:    $SKILL_PATH"
echo "  Settings: $CLAUDE_SETTINGS"
echo ""
echo "  → Restart Claude Code to activate"
echo ""
echo "  First use:"
echo "  Open Claude Code in any project and"
echo "  send a prompt — the workspace"
echo "  initializes automatically."
echo ""
echo "  To add facts manually:"
echo "  engram curate --sync \"your fact here\""
echo "═══════════════════════════════════════"
