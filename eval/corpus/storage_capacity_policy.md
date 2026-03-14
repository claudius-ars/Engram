---
title: "Storage Capacity Management Policy"
factType: durable
tags:
  - storage
  - capacity
  - monitoring
keywords:
  - disk-usage
  - alerting
  - retention
  - archive
domainTags:
  - infra:database
confidence: 0.9
importance: 0.85
recency: 0.9
causedBy:
  - incident_db_outage
---

## Raw Concept

Storage capacity management requires alerting at three thresholds: warning at 70% utilization, critical at 85%, and emergency at 95%. All alerts must route to both the on-call PagerDuty rotation and the infrastructure Slack channel, with the emergency threshold triggering an automatic page regardless of quiet hours. Data retention policies mandate that transactional data older than 90 days is archived to S3 Glacier, application logs older than 30 days are deleted, and database backups are retained for 90 days with weekly snapshots kept for 1 year. Temporary tables and materialized views must have explicit TTL annotations; any temporary object without a TTL is flagged in the weekly capacity review. Capacity forecasting runs monthly using linear regression on the trailing 90-day growth rate.
