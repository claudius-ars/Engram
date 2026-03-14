---
title: "Rust Error Handling Patterns"
factType: durable
tags:
  - rust
  - programming
keywords:
  - error-handling
  - Result
  - anyhow
confidence: 0.98
importance: 0.9
recency: 1.0
---

## Raw Concept

Prefer `thiserror` for library crates (structured, typed errors) and
`anyhow` for application crates (ad-hoc context). Use the `?` operator
for propagation. Avoid `.unwrap()` in production code paths.
