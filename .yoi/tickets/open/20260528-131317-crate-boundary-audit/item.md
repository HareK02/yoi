---
id: 20260528-131317-crate-boundary-audit
slug: crate-boundary-audit
title: Audit crate responsibility boundaries
status: open
kind: audit
priority: P2
labels: [architecture, crates]
workflow_state: planning
created_at: 2026-05-28T13:13:17Z
updated_at: 2026-05-28T13:13:17Z
assignee: null
---

## Background

The workspace has grown across multiple crates (`pod`, `protocol`, `llm-worker`, `manifest`, `client`, `tui`, `memory`, `workflow`, etc.). Before adding more orchestration and policy features, audit whether crate responsibilities, dependency direction, and public interfaces are still clean.

This is an architecture audit, not an implementation ticket. The output should be actionable findings: either concrete boundary violations to fix, or an explicit statement that the inspected area is acceptable.

The audit must also check code comments and documentation comments. Comments inside one crate should not explain or justify behavior primarily in terms of a downstream crate that depends on it. If such comments exist, record them because they can indicate inverted ownership or an interface that is leaking caller-specific concerns.

## Scope

Inspect the Rust workspace at least for:

- crate dependency graph and suspicious dependency direction.
- public types/functions/modules whose names or contracts expose another crate's implementation details unnecessarily.
- code paths where a lower-level crate appears to know about higher-level orchestration, UI, or caller concerns.
- comments/doc-comments that mention another crate which depends on the current crate, especially when the comment describes why the dependent crate needs that behavior.
- duplicated interfaces or ad-hoc glue that should be owned by a clearer boundary.

Out of scope:

- broad refactoring.
- formatting-only changes.
- changing dependency direction before findings are reviewed.
- rewriting comments unless a follow-up implementation ticket is explicitly created.

## Acceptance criteria

- A dependency/interface audit summary exists with concrete findings grouped by severity.
- The audit names files/modules/functions/comments involved in each finding.
- The audit distinguishes actual boundary problems from acceptable dependency-aware documentation.
- The audit specifically reports whether comments in crates refer to crates that depend on them.
- If no blocking issue is found, the audit explains why the current separation is acceptable.
- Follow-up implementation tickets are proposed only for findings that are specific and actionable.
