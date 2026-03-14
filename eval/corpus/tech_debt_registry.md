---
title: "Technical Debt Registry Current State"
factType: state
tags:
  - tech-debt
  - engineering
  - planning
keywords:
  - priority
  - registry
  - review
domainTags: []
confidence: 0.85
importance: 0.7
recency: 0.9
validUntil: "2099-12-31T00:00:00Z"
---

## Raw Concept

The technical debt registry currently tracks 30 items: 7 high priority, 23 medium priority, and 0 low priority (low items are pruned quarterly). The top three high-priority items are: (1) replace the legacy event bus with NATS JetStream (estimated 3 sprints, blocked by client library maturity), (2) migrate the authentication middleware from the deprecated session-token model to JWT with PKCE (estimated 2 sprints, compliance deadline Q3 2024), and (3) consolidate the three separate configuration loading paths into a single unified config crate (estimated 1 sprint). The registry was last reviewed on March 8, 2024 at the architecture review board meeting. Each item is scored using a cost-of-delay model that factors in risk of incident, developer productivity impact, and blocking dependencies. New tech debt items are submitted via a template in the engineering wiki and triaged biweekly.
