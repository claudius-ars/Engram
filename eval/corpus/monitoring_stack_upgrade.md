---
title: "Monitoring Stack Upgrade March 2024"
factType: event
tags:
  - monitoring
  - observability
  - upgrade
keywords:
  - prometheus
  - grafana
  - alerting
domainTags:
  - infra:observability
confidence: 1.0
importance: 0.75
recency: 0.9
eventSequence: 1
createdAt: "2024-03-05T09:00:00Z"
---

## Raw Concept

The monitoring stack was upgraded on March 5, 2024: Prometheus from v2.48 to v2.50 and Grafana from v10.2 to v10.4. The Prometheus upgrade introduced native histogram support, which was immediately adopted for API latency tracking — reducing cardinality by 60% compared to the previous explicit bucket configuration. New alerting rules were added for storage capacity (triggered by the February outage), including predictive alerts that fire when disk usage growth rate projects 95% utilization within 7 days. Grafana dashboards were migrated to the new Scenes framework for improved rendering performance. The upgrade required a 3-minute Prometheus restart window during which metrics collection paused, but no data loss occurred due to the remote-write buffer to Thanos.
