---
title: "Q1 2024 Kubernetes Cluster Migration"
factType: event
tags:
  - kubernetes
  - migration
  - infrastructure
keywords:
  - cluster
  - upgrade
  - zero-downtime
domainTags:
  - infra:k8s
confidence: 0.95
importance: 0.85
recency: 0.9
eventSequence: 1
causes:
  - kubernetes_scheduling
createdAt: "2024-01-15T06:00:00Z"
---

## Raw Concept

The production Kubernetes cluster was migrated from self-managed v1.26 to EKS v1.28 during Q1 2024. The migration added 3 new m6i.2xlarge nodes to the cluster, bringing the total to 12 nodes across 3 availability zones. A zero-downtime rollout was achieved using a blue-green deployment strategy — the new cluster was provisioned alongside the old one, traffic was gradually shifted using weighted DNS records, and the old cluster was decommissioned after 72 hours of parallel operation. Pod scheduling rules were updated to leverage EKS-native features including Karpenter for node autoscaling, replacing the previous cluster-autoscaler configuration. The migration resolved persistent node-pressure eviction issues that had been affecting batch workloads.
