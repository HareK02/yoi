---
id: 20260601-123641-dependency-license-audit
slug: dependency-license-audit
title: Audit external dependencies and license posture
status: open
kind: task
priority: P2
labels: [audit, dependencies, license]
created_at: 2026-06-01T12:36:41Z
updated_at: 2026-06-01T12:37:30Z
assignee: null
legacy_ticket: null
---

## Background

Before public MIT release, Yoi needs a focused audit of external dependencies and their licenses. The goal is not to remove every dependency, but to identify heavy or weakly-justified dependencies, dependencies that are easy to replace with simpler code or existing transitive functionality, and any licensing or notice obligations that conflict with an MIT release posture.

This is an investigation/audit ticket. It should produce an actionable report rather than immediate dependency changes.

## Requirements

- Inventory Rust crate dependencies from `Cargo.lock` / workspace metadata, including direct vs transitive status where practical.
- Identify direct dependencies that appear heavy, barely used, redundant, or plausibly replaceable.
- Check license metadata for direct and transitive Rust dependencies and flag unknown, copyleft, non-standard, or notice-relevant licenses.
- Inspect Nix/runtime/system dependencies declared by `flake.nix`, `package.nix`, and `devshell.nix` at a high level.
- Distinguish release blockers from advisory cleanup opportunities.
- Do not read ignored secret-like file contents.
- Do not modify dependency manifests as part of this ticket unless explicitly approved later.

## Acceptance criteria

- An artifact report is written under this ticket with:
  - dependency inventory methodology;
  - direct dependency list with rough purpose/usage notes;
  - heavy/redundant/replaceable dependency candidates;
  - license compatibility findings for an MIT publication posture;
  - Nix/system dependency notes;
  - recommended follow-up tickets, if any.
- Report clearly marks any release blockers vs non-blocking cleanup.
- Validation/evidence commands used for the audit are recorded.
