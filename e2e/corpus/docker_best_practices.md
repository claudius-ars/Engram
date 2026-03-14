---
title: "Docker Image Best Practices"
factType: durable
tags:
  - docker
  - containers
keywords:
  - multi-stage
  - layer-caching
domainTags:
  - infra:docker
confidence: 0.92
importance: 0.75
recency: 0.95
---

## Raw Concept

Use multi-stage builds to minimize final image size. Order
Dockerfile instructions from least to most frequently changing
to maximize layer cache hits. Pin base image digests in production.
