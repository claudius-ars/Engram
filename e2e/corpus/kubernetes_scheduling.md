---
title: "Kubernetes Pod Scheduling"
factType: durable
tags:
  - kubernetes
  - infrastructure
keywords:
  - scheduling
  - pod-affinity
domainTags:
  - infra:k8s
confidence: 0.95
importance: 0.8
recency: 1.0
causedBy:
  - cluster_migration
---

## Raw Concept

Kubernetes pod scheduling uses a two-phase process: filtering
(which nodes can run the pod) and scoring (which node is best).
Pod affinity rules allow co-locating related workloads.
