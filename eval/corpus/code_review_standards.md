---
title: "Code Review Standards and Merge Policy"
factType: durable
tags:
  - code-review
  - engineering
  - process
keywords:
  - PR
  - approval
  - merge
  - automated-checks
domainTags: []
confidence: 0.9
importance: 0.7
recency: 1.0
---

## Raw Concept

All production code changes require a pull request with at least two approvals from team members, one of whom must be a designated code owner for the affected module. Automated checks must pass before merge: CI build, full test suite, clippy lint (zero warnings), cargo fmt verification, and security scan. PRs touching database migrations require an additional DBA review. The merge strategy uses squash-and-merge for feature branches and merge commits for release branches to preserve individual commit history. PRs should be scoped to a single logical change — combined refactoring and feature work must be split into separate PRs. Review turnaround SLA is 24 hours for normal PRs and 4 hours for security patches. Stale PRs (no activity for 7 days) are automatically flagged and closed after 30 days.
