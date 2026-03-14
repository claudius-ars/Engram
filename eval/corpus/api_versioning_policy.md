---
title: "REST API Versioning Policy"
factType: durable
tags:
  - api
  - rest
  - versioning
keywords:
  - deprecation
  - backward-compatibility
  - URL-path
domainTags: []
confidence: 0.9
importance: 0.75
recency: 1.0
---

## Raw Concept

APIs are versioned using URL path segments (e.g., /v1/wells, /v2/wells). Major version increments are reserved for breaking changes — field removals, type changes, or semantic modifications to existing endpoints. Minor enhancements (new optional fields, additional endpoints) are added within the current major version without incrementing. Deprecated API versions receive security patches only for 12 months after the successor version reaches general availability, then are removed with 90 days notice. All API responses include a Deprecation header when the requested version has a newer successor available. Clients must specify an explicit version; requests without a version prefix return 404. The API gateway enforces rate limits per version independently to prevent deprecated version traffic from impacting current version capacity.
