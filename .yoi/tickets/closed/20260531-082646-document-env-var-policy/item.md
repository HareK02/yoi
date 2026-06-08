---
id: 20260531-082646-document-env-var-policy
slug: document-env-var-policy
title: 'Docs: document environment variable policy'
status: closed
kind: task
priority: P2
labels: [docs, config, security]
created_at: 2026-05-31T08:26:46Z
updated_at: 2026-05-31T08:29:40Z
assignee: null
---

## Background

Environment variables are currently used for a few practical boundaries: XDG-style path discovery, runtime/socket directories, development overrides, and legacy/provider secret inputs. The user's preference is that this project should avoid environment variables where possible and make any remaining environment-variable surface explicit.

A short investigation found that path resolution is mostly centralized in `manifest::paths`, while auth/web secret envs and test-only env mutation are more scattered. Normal runtime intentionally does not implicitly load `.env` files.

## Requirements

- Add current documentation for environment-variable policy and supported variables.
- State the design preference clearly: avoid new environment variables when manifest/profile/config/typed secret references are better.
- Document the currently supported categories:
  - core path/resource discovery;
  - runtime/socket/registry discovery;
  - Pod runtime command development override;
  - provider/WebSearch credential references;
  - external compatibility variables such as Codex home;
  - test/build/example-only environment variables.
- Clarify that normal runtime must not implicitly load `.env` files.
- Identify cleanup direction without implementing unrelated refactors in this ticket.

## Acceptance criteria

- A user/developer-facing docs page explains environment-variable policy and current variables.
- Existing Nix/config docs link to the new policy page where relevant.
- Documentation does not expose secret values or read ignored secret-like files.
- `./tickets.sh doctor` and `git diff --check` pass.
