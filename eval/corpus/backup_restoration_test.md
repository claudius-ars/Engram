---
title: "Quarterly Backup Restoration Drill Q1 2024"
factType: event
tags:
  - backup
  - disaster-recovery
  - testing
keywords:
  - restoration
  - RTO
  - RPO
  - drill
domainTags:
  - infra:database
confidence: 1.0
importance: 0.8
recency: 0.85
eventSequence: 1
causedBy:
  - incident_db_outage
createdAt: "2024-03-22T10:00:00Z"
---

## Raw Concept

The Q1 2024 quarterly backup restoration drill was conducted on March 22, 2024, motivated by the February database outage. The drill restored the production PostgreSQL database from the most recent daily snapshot to a staging environment. Recovery Time Objective (RTO) was measured at 22 minutes from initiation to a fully operational database accepting queries. Recovery Point Objective (RPO) was 4 hours, representing the gap between the last backup snapshot and the simulated failure point. The drill validated that the automated restoration playbook works correctly with the new S3 backup storage path configured after the infrastructure migration. One issue was identified: the post-restoration schema migration step took 8 minutes longer than expected due to index rebuilds on three large tables.
