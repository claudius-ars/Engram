# Engram Memory Recall

Use this skill to query the project's Engram knowledge base or to save
new facts for future sessions.

## When to Use This Skill

**Query memory when:**
- Asked about project decisions, conventions, or past events
- Working in an unfamiliar area of the codebase
- The user asks "do we have notes on..." or "what did we decide about..."
- A task requires context that may exist in the knowledge base

**Save to memory when:**
- The user explicitly asks you to remember something
- A significant architectural decision is made during the session
- A constraint or convention is discovered that future sessions should know
- The user says "remember this", "save this", "add this to memory"

## Querying Memory

Run the following command, replacing QUERY with the search terms:

```bash
cd "$ENGRAM_WORKSPACE" || cd "$CLAUDE_PROJECT_DIR"
engram query --format json --agent "claude-code" "QUERY"
```

Parse the NDJSON output. Each line is a JSON object with these fields:
- `title` — fact title
- `fact_id` — unique identifier
- `body` — fact content (up to 500 characters)
- `fact_type` — durable, state, or event
- `score` — relevance score
- `tags` — domain tags

Present results as a concise summary. Do not dump raw JSON to the user.
If no results are found, say so clearly.

## Saving to Memory (Curating)

To save a fact, run:

```bash
cd "$ENGRAM_WORKSPACE" || cd "$CLAUDE_PROJECT_DIR"
engram curate --sync "FACT_SUMMARY"
```

The `--sync` flag compiles immediately so the fact is queryable in the
same session.

**Curate guidelines:**
- Write facts as declarative statements, not questions or commands
- Include enough context to be useful without the conversation history
- For decisions: include the decision AND the reasoning
- For conventions: include the rule AND when it applies
- Keep each fact focused on a single topic

**Good curate examples:**
```bash
engram curate --sync "The team uses thiserror for library crates and anyhow for binaries. This was decided in Q1 2024 to keep error types clean at crate boundaries."

engram curate --sync "Well B-7 has active sustained casing pressure of 450 psi on the A-annulus. Workover scheduled Q3 2024. Monitor daily."

engram curate --sync "The data pipeline Rust rewrite achieved 40% throughput improvement over the Python implementation. Completed Q1 2024."
```

**Bad curate examples (do not do this):**
```bash
# Too vague
engram curate --sync "We talked about Rust"

# Conversational, not declarative
engram curate --sync "The user asked me to explain error handling"

# Already in the codebase — code is self-documenting
engram curate --sync "The main function is in src/main.rs"
```

## Checking What's in Memory

To list recently indexed facts:

```bash
cd "$ENGRAM_WORKSPACE" || cd "$CLAUDE_PROJECT_DIR"
engram query --format json "." | head -20
```

## Environment

- `ENGRAM_WORKSPACE` — path to the .brv workspace (may be unset;
  fall back to `CLAUDE_PROJECT_DIR`)
- `ENGRAM_BIN` — path to engram binary (may be unset; use `engram`
  from PATH)

If neither workspace nor binary is available, tell the user that Engram
is not configured for this project and explain how to set it up.

## Setup (If Not Configured)

If `.brv/` does not exist in the project directory, Engram is not yet
initialized. Tell the user:

1. Add fact files to `.brv/context-tree/` as Markdown with YAML
   frontmatter
2. Run `engram compile` to build the index
3. Or use `engram curate --sync "..."` to create the first fact
   (it will initialize the workspace automatically)
