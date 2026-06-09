---
title: "Companion status context and tool policy"
state: "planning"
created_at: "2026-06-07T00:16:51Z"
updated_at: "2026-06-07T02:45:32Z"
---

## Background

The workspace Companion should help the human understand and steer the workspace, not act as a direct implementation worker. It should know what is complete, what remains, what is queued/in progress, which Pods exist, and where attention may be useful. It should not directly edit files, mutate Tickets, spawn implementation Pods, or perform destructive actions.

This requires a distinct prompt/profile/tool policy from ordinary coder/reviewer/orchestrator Pods.

## Goal

Define and implement the Companion's prompt/profile/tool policy so it can provide real-time situational assistance while being prevented from direct write/mutation authority.

## Requirements

- Add or define a Companion role/profile/prompt for workspace panel usage.
- System prompt should instruct the Companion to:
  - summarize current workspace status;
  - explain completed/remaining work;
  - help the human understand Ticket/Pod/Orchestrator state;
  - suggest next safe high-level actions;
  - avoid directly implementing, editing, or mutating project state.
- Tool policy should prohibit direct file writes and direct Ticket mutation by default.
- Tool policy should prohibit spawning implementation/review Pods directly unless a later explicit design grants that authority.
- Companion may have read/status capabilities needed for situational awareness, such as bounded Ticket list/show, Pod list/state, and possibly read-only docs/context access.
- If read-only status is provided through a specialized summary/context mechanism rather than raw tools, keep it auditable and derived from authoritative Ticket/Pod state.
- Companion should not receive hidden, non-history context that affects behavior without being committed to an appropriate history/event path.
- Keep secrets/private input out of diagnostics, status summaries, and prompts.
- Preserve Orchestrator as the actor responsible for scheduling/routing work; Companion is human-facing support, not scheduler authority.

## Non-goals

- Giving Companion write access.
- Replacing Orchestrator.
- Replacing Ticket tools/workflows.
- Building a full scheduler.
- Final UI layout tuning.

## Acceptance criteria

- A Companion prompt/profile/tool policy exists and is used by `yoi panel` Companion lifecycle.
- Companion can answer status questions from read-only/derived authoritative sources.
- Companion cannot directly mutate repository files or Ticket records under default policy.
- Companion cannot directly launch implementation/review Pods under default policy.
- Prompt/tool docs clearly distinguish Companion from Orchestrator, Intake, coder, and reviewer roles.
