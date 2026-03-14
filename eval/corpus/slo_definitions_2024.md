---
title: "Service Level Objectives 2024"
factType: durable
tags:
  - slo
  - observability
  - reliability
keywords:
  - latency
  - uptime
  - error-rate
  - SLA
domainTags:
  - infra:observability
confidence: 0.95
importance: 0.9
recency: 1.0
---

## Raw Concept

Service level objectives for 2024 define three primary reliability targets. Availability: the API must maintain 99.9% uptime measured over rolling 30-day windows, with planned maintenance excluded from the calculation. Latency: p99 response time for the query API must remain below 200ms, measured at the load balancer level. Error rate: the 5xx error rate must stay below 0.1% of total requests. SLO burn rate alerts are configured at 2x (warning, 1-hour window) and 10x (critical, 5-minute window) thresholds. Error budget consumption is reviewed weekly in the reliability standup. When the monthly error budget is exhausted, new feature deployments are frozen until the budget resets or the underlying issue is resolved. These SLOs map to the external SLA commitment of 99.5% availability with financial penalties.
