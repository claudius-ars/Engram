---
title: "PostgreSQL 17 Incremental Backup Support"
factType: durable
tags:
  - postgresql
  - backup
  - database
keywords:
  - incremental
  - pg_basebackup
  - WAL
  - archiving
domainTags: []
confidence: 0.88
importance: 0.7
recency: 0.9
---

## Raw Concept

PostgreSQL 17 introduces native incremental backup support through the pg_basebackup tool, eliminating the need for third-party solutions like pgBackRest for incremental backup chains. The new --incremental flag allows pg_basebackup to reference a prior backup manifest and only transfer changed blocks, reducing backup size by 70-90% for typical workloads. WAL archiving remains required for point-in-time recovery between incremental snapshots. The backup strategy combines daily incremental backups with weekly full backups, retaining 4 weekly full backups and all intervening incrementals. Restoration requires the full base backup plus all subsequent incrementals applied in order. The team plans to adopt this feature once PostgreSQL 17 reaches the second minor release, replacing the current pgBackRest configuration that handles approximately 800GB of database across 3 instances.
