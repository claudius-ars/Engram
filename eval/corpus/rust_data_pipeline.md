---
title: "Rust Data Pipeline Rewrite"
factType: durable
tags:
  - rust
  - data-pipeline
  - performance
keywords:
  - throughput
  - memory-safety
  - rewrite
domainTags: []
confidence: 0.92
importance: 0.85
recency: 0.95
---

## Raw Concept

The data ingestion pipeline was rewritten from Python to Rust in Q1 2024, achieving a 40% improvement in throughput from 12,000 to 16,800 events per second on the same hardware. The primary motivation was memory safety — the Python pipeline had experienced three out-of-memory crashes in production due to unbounded buffering in the transformation stage. The Rust implementation uses bounded channels with backpressure, ensuring memory usage stays within a 2GB ceiling regardless of input volume. The pipeline processes OSDU-format well data records, applying schema validation, unit conversion, and deduplication before writing to the time-series database. Zero-copy deserialization using serde reduces allocation overhead for the parsing stage. The team adopted Rust specifically for this component rather than a full rewrite, keeping the API layer in Python.
