---
title: "Production Deploy v2.2"
factType: event
tags:
  - deployment
  - production
keywords:
  - release
  - hotfix
causedBy:
  - deploy_event_1
confidence: 1.0
importance: 0.85
recency: 0.8
eventSequence: 11
createdAt: "2024-03-01T10:00:00Z"
---

## Raw Concept

Hotfix deploy for v2.2 addressing a race condition in the rate limiter
introduced in v2.1. Fix involved switching from a shared counter to
per-worker atomic counters.
