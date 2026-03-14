---
title: "Current Production Deployment v2.4.1"
factType: state
tags:
  - deployment
  - production
  - kubernetes
keywords:
  - version
  - rollout
  - healthy
domainTags:
  - infra:k8s
confidence: 1.0
importance: 0.8
recency: 1.0
validUntil: "2099-12-31T00:00:00Z"
---

## Raw Concept

The production cluster is currently running application version v2.4.1, deployed on March 10, 2024 via ArgoCD with automated canary analysis. All 12 nodes report healthy status with no pending maintenance operations. The deployment includes 47 pods across 8 namespaces, with resource utilization averaging 62% CPU and 71% memory across the cluster. The last rollout completed in 18 minutes with zero error-rate increase during the canary phase. Two feature flags were enabled in this release: the new caching layer for the query API and rate limiting on the public endpoint. The previous version (v2.4.0) is retained as a rollback target in the ArgoCD history.
