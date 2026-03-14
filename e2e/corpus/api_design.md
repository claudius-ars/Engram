---
title: "REST API Design Guidelines"
factType: durable
tags:
  - api
  - rest
keywords:
  - http
  - endpoints
  - versioning
confidence: 0.88
importance: 0.7
recency: 0.9
---

## Raw Concept

Use plural nouns for resource endpoints (/users not /user).
Version APIs in the URL path (/v1/users). Return 201 for
resource creation, 204 for successful deletes. Use HATEOAS
links for discoverability.
