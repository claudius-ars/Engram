---
title: "Database Outage Incident February 2024"
factType: event
tags:
  - incident
  - database
  - outage
keywords:
  - downtime
  - disk-exhaustion
  - postmortem
domainTags:
  - infra:database
confidence: 1.0
importance: 0.9
recency: 0.85
eventSequence: 1
causedBy:
  - storage_capacity_policy
createdAt: "2024-02-28T03:15:00Z"
---

## Raw Concept

A production database outage occurred on February 28, 2024 at 03:15 UTC, lasting 47 minutes. The root cause was disk exhaustion on the primary PostgreSQL server — the data volume reached 100% capacity due to an unmonitored temporary table used by a nightly ETL job that had been growing by 2GB per day for three weeks. The database server entered read-only mode, causing cascading failures in 6 downstream services. Recovery was achieved by manually dropping the temporary table, reclaiming 42GB of disk space, and restarting the affected services. The incident exposed gaps in the storage capacity monitoring: the alerting threshold was set at 90% but the alert channel had been misconfigured during a previous Slack migration and was routing to a decommissioned channel.
