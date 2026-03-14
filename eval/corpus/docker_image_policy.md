---
title: "Docker Image Security Policy"
factType: durable
tags:
  - docker
  - security
  - containers
keywords:
  - distroless
  - vulnerability-scanning
  - registry
  - base-image
domainTags:
  - infra:docker
confidence: 0.93
importance: 0.8
recency: 1.0
---

## Raw Concept

All production container images must use Google distroless base images or approved Alpine variants pinned by digest (not tag). Images are scanned for vulnerabilities on every push to the internal registry using Trivy, with a gate that blocks deployment of images containing any critical or high-severity CVE older than 30 days. Multi-stage builds are mandatory to ensure build tools and source code are excluded from the runtime image. The maximum allowed image size for microservices is 150MB; exceptions require architecture review approval. Images must include OCI labels for maintainer, build timestamp, and git SHA. The internal registry enforces image signing via Cosign, and unsigned images are rejected by the admission controller in all production namespaces.
