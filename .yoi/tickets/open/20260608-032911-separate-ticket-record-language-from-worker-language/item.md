---
id: '20260608-032911-separate-ticket-record-language-from-worker-language'
slug: 'separate-ticket-record-language-from-worker-language'
title: 'Separate Ticket record language from worker response language'
status: 'open'
kind: 'task'
priority: 'P2'
labels: ['ticket', 'language', 'config', 'profile', 'workflow']
workflow_state: 'inprogress'
created_at: '2026-06-08T03:29:11Z'
updated_at: '2026-06-08T07:41:11Z'
assignee: null
legacy_ticket: null
queued_by: 'workspace-panel'
queued_at: '2026-06-08T05:51:29Z'
---

## Background

Profiles/Manifests currently provide `worker.language`, which controls normal Pod prose responses. This is not the same as the language used for durable Ticket records.

Ticket records are authored by multiple paths:

- LLM role Pods writing Ticket comments/reviews/decisions;
- typed Ticket tools generating state-change/intake/close events;
- Panel/CLI generated notices/resolutions;
- scaffold/default Ticket bodies.

This project wants to distinguish:

- response language: how a Pod replies to the user in conversation;
- Ticket record language: the language used for durable Ticket `item.md`, `thread.md`, `resolution.md`, and generated Ticket event bodies.

Currently `.yoi/ticket.config.toml` has no language setting. Role profiles can set `worker.language = "Japanese"`, but that does not provide a backend/tool-level policy for machine-generated Ticket text.

## Goal

Add an explicit Ticket record language configuration separate from Pod response language.

## Requirements

- Add a Ticket config setting for durable Ticket record language.
  - Candidate shape:

```toml
[ticket]
language = "Japanese"
```

  - Avoid overloading `[worker].language` or Profile settings for this purpose.
- Define the boundary clearly:
  - `worker.language`: conversation/prose responses by a Pod.
  - `ticket.language`: durable Ticket records and generated Ticket event/resolution/scaffold text.
- Make typed Ticket tool/backend generated text consult `ticket.language` where practical.
- Ensure LLM role prompts can receive or reference Ticket record language so comments/reviews/decisions use the configured Ticket language even if the Pod response language differs.
- Preserve existing behavior when `ticket.language` is absent, using the current/default language policy.
- Update `.yoi/ticket.config.toml` scaffold and parser.
- Update project `.yoi/ticket.config.toml` to set the desired Ticket record language.
- Avoid translating protocol literals, file paths, commands, logs, identifiers, or quoted external text merely because Ticket language is set.

## Design questions

- Should the config key be `[ticket].language`, `[records].language`, or `[backend].language`?
  - Prefer `[ticket].language` because this is record-generation policy, not storage backend mechanics.
- Should tool-generated default bodies/resolutions be localized immediately, or should this ticket only thread the config through and update the most visible generation paths?
- How should existing Tickets be handled? Do not rewrite existing records unless explicitly requested.

## Acceptance criteria

- Ticket config can specify a Ticket record language independently of Profile/worker language.
- Role launch prompts/tool instructions expose the Ticket record language to Intake/Orchestrator/Coder/Reviewer where they write Ticket records.
- Typed Ticket-generated events/resolutions/scaffold have a clear policy for using the configured language.
- Absence of `ticket.language` preserves current behavior.
- Existing Tickets are not bulk-translated or rewritten.
- Tests cover config parsing/scaffold/default behavior and at least one generation/role prompt path.
- `target/debug/yoi ticket doctor`, focused tests, `cargo fmt --check`, and `git diff --check` pass.
