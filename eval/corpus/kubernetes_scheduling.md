---
title: "Kubernetes Pod Scheduling Architecture"
factType: durable
tags:
  - kubernetes
  - infrastructure
  - scheduling
keywords:
  - pod-affinity
  - taints
  - tolerations
  - node-selector
domainTags:
  - infra:k8s
confidence: 0.95
importance: 0.85
recency: 1.0
causedBy:
  - cluster_migration_q1
---

## Raw Concept

Kubernetes pod scheduling follows a two-phase architecture: filtering and scoring. During filtering, the scheduler eliminates nodes that cannot run the pod based on resource requests, node selectors, taints and tolerations, and pod affinity/anti-affinity rules. During scoring, remaining candidate nodes are ranked using priority functions including least-requested resources, balanced resource allocation, and inter-pod affinity. Custom scheduling profiles can be configured for workload-specific placement — the data pipeline pods use a dedicated profile that favors nodes with NVMe storage, while API server pods prefer nodes in availability zones with low network latency. Taints are used to reserve GPU nodes exclusively for ML training workloads.
