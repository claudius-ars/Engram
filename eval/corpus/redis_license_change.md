---
title: "Redis License Change to Dual-License"
factType: event
tags:
  - redis
  - licensing
  - open-source
keywords:
  - RSALv2
  - SSPL
  - dual-license
  - valkey
domainTags: []
confidence: 0.95
importance: 0.8
recency: 0.85
eventSequence: 1
createdAt: "2024-03-20T00:00:00Z"
---

## Raw Concept

Redis Labs changed the Redis license from BSD 3-Clause to a dual-license model (RSALv2 and SSPLv1) effective with version 7.4, announced in March 2024. This license change affects any organization that provides Redis as a managed service or embeds Redis in a commercial product. For internal deployments, the RSALv2 license permits continued use without modification. The team evaluated three alternatives: (1) continuing with Redis under the new license since our use case is internal-only, (2) migrating to Valkey, the Linux Foundation fork that maintains the BSD license, and (3) switching to KeyDB or Dragonfly. The decision was to remain on Redis for now with a contingency plan to migrate to Valkey if the licensing terms change further or if the community fork demonstrates production stability.
