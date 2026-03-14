---
title: "Rust Error Handling Patterns"
factType: durable
tags:
  - rust
  - programming
  - error-handling
keywords:
  - Result
  - anyhow
  - thiserror
  - error-boundary
domainTags: []
confidence: 0.98
importance: 0.85
recency: 1.0
---

## Raw Concept

The codebase follows a two-tier error handling strategy. Library crates use thiserror to define structured, typed error enums that give callers precise control over error matching and recovery. Application crates (CLI, API server) use anyhow for ad-hoc error context, converting library errors at the boundary using the ? operator with .context() annotations. The boundary between library and application error handling is the public API surface of each crate. Structured error logging uses tracing::error! with key-value fields rather than format strings, enabling machine-parseable error output. Panics are reserved for invariant violations in internal logic (using assert! and unreachable!); all I/O operations, parsing, and external calls must return Result. The unwrap() method is banned in production code paths via a clippy lint configuration.
