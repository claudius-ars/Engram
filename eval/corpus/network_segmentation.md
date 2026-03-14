---
title: "Network Segmentation and Service Mesh Policy"
factType: durable
tags:
  - networking
  - security
  - service-mesh
keywords:
  - VLAN
  - mTLS
  - east-west
  - zero-trust
domainTags:
  - infra:networking
confidence: 0.92
importance: 0.85
recency: 1.0
---

## Raw Concept

The network architecture follows a zero-trust model with strict east-west traffic segmentation. Production workloads are isolated in VLAN 100, staging in VLAN 200, and development in VLAN 300, with no direct routing between environments. Inter-service communication within production uses Istio service mesh with mandatory mTLS — plaintext connections are rejected by sidecar proxies. Network policies enforce a default-deny posture: each service must explicitly declare its ingress and egress dependencies via Kubernetes NetworkPolicy resources. External traffic enters through a dedicated DMZ VLAN via an F5 load balancer, with WAF rules applied before routing to the Istio ingress gateway. DNS resolution between environments is blocked at the CoreDNS level to prevent accidental cross-environment service discovery.
