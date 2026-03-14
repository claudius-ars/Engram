# Engram Claude Code Plugin

Memory-augmented Claude Code sessions powered by Engram. This plugin
automatically retrieves relevant facts from the project knowledge base on
every prompt, and provides a skill for on-demand memory recall and curation.

## Prerequisites

- **Engram binary** — built and available in `PATH`, or set `ENGRAM_BIN`
- **Initialized workspace** — a `.brv/` directory with a compiled index
  (run `engram compile` to create one)
- **Claude Code** — v1.0+ with plugin support

## Installation

The plugin installs by wiring its hooks into Claude Code's settings.
There is no marketplace registration step — this is a local plugin
that lives alongside the Engram binary.

### Option A — User-scoped (recommended)

Install once, works in every project automatically. In projects
without a `.brv/` workspace the plugin does nothing silently.

Add the hooks block to your user-level Claude Code settings file:

- **macOS:** `~/.claude/settings.json`
- **Linux:** `~/.config/claude/settings.json`

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
to this directory. **Absolute paths are required.**

If the settings file already has content, merge the `hooks` block
into the existing JSON rather than replacing the file.

### Option B — Project-scoped

Install per-project by adding the same hooks block to
`.claude/settings.json` in the project root. Use this when you want
Engram active only in specific projects.

### After installing — Restart Claude Code

Hook changes take effect after restarting Claude Code. If you edited
settings during an active session, restart before testing.

### Verify

Test the retrieval hook directly:

```bash
echo "how does the query pipeline work" | \
    CLAUDE_PROJECT_DIR=/path/to/your/project \
    ENGRAM_BIN=$(which engram) \
    /absolute/path/to/engram-claudecode/hooks/user_prompt_submit.sh
```

Expected output: an `## Engram Memory Context` block with matching
facts. If output is empty, run the smoke test to diagnose:

```bash
/absolute/path/to/engram-claudecode/test_hook.sh /path/to/your/project
```

### Note on `plugin.json`

The `.claude-plugin/plugin.json` manifest is included for future
compatibility when Anthropic adds local directory plugin support to
the Claude Code CLI. It is not used by the current installation
method.

## How It Works

### Retrieval (automatic)

On every `UserPromptSubmit` event:

1. The hook reads the user's prompt from stdin.
2. Prompts shorter than 10 characters are skipped (no retrieval for "hi").
3. If the Engram index is dirty, an incremental compile runs first.
4. `engram query --format json --agent claude-code` searches the index.
5. Results are formatted into a Markdown context block.
6. The context block is written to stdout — Claude Code prepends it to the
   conversation so the model sees relevant facts before responding.

### Curation reminder (automatic)

When a Claude Code session ends (the `Stop` event), the plugin prints a
reminder to curate any facts worth remembering. This nudges the user (or
the model) to persist knowledge before the session context is lost.

### On-demand recall (skill)

The `memory-recall` skill lets the model query or curate facts explicitly:

- **Query**: `engram query --format json --agent claude-code "search terms"`
- **Save**: `engram curate --sync "declarative fact statement"`

The skill activates when the user asks about project decisions, conventions,
or past events, or when the user says "remember this".

## Components

```
engram-claudecode/
├── .claude-plugin/
│   └── plugin.json            # Plugin manifest
├── hooks/
│   ├── hooks.json             # Hook wiring (UserPromptSubmit + Stop)
│   ├── user_prompt_submit.sh  # Retrieval hook
│   └── session_end.sh         # Curation reminder hook
├── scripts/
│   ├── check_and_compile.sh   # Dirty-index detection + incremental compile
│   └── format_results.sh      # NDJSON → Markdown formatter
├── skills/
│   └── memory-recall/
│       └── SKILL.md           # On-demand query + curate skill
├── test_hook.sh               # Smoke tests (8 cases)
└── README.md                  # This file
```

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `ENGRAM_BIN` | `engram` from PATH | Path to the engram binary |
| `ENGRAM_WORKSPACE` | `$CLAUDE_PROJECT_DIR` | Path to workspace root containing `.brv/` |
| `ENGRAM_AGENT_ID` | `claude-code` | Agent ID for Bulwark policy and access logging |
| `ENGRAM_MAX_RESULTS` | `3` | Maximum facts to inject per prompt |

## Testing

Run the smoke test suite:

```bash
cd engram-claudecode
./test_hook.sh /path/to/workspace/with/.brv
```

The test suite covers 8 cases:

1. Short prompt — no output
2. Real query — context block with body content
3. Missing binary — graceful exit 0
4. Missing workspace — graceful exit 0
5. "No results found" — formatter produces no output
6. Empty input — formatter produces no output
7. Session end with workspace — reminder printed
8. Session end without workspace — no output

All hooks exit 0 in every case. A non-zero exit from a `UserPromptSubmit`
hook would block the user's prompt; a non-zero exit from a `Stop` hook
would abort the session.

## Design Principles

- **Zero-cost when idle** — if there's no `.brv/` workspace, all hooks
  exit immediately with no output.
- **Fail-open** — missing binary, empty index, query errors: everything
  degrades to silence rather than blocking the user.
- **No network calls** — all retrieval is local (Tantivy index on disk).
- **Incremental compile** — dirty indexes are rebuilt automatically before
  querying, so the user always gets fresh results.
- **Body text at display time** — fact bodies are read from source `.md`
  files when formatting results, not stored in the search index (preserving
  the NRD-4 design principle).
