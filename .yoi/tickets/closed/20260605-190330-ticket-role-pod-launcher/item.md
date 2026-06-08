---
id: 20260605-190330-ticket-role-pod-launcher
slug: ticket-role-pod-launcher
title: Ticket role Pod launcher
status: closed
kind: task
priority: P1
labels: [ticket, pod, role, orchestration]
created_at: 2026-06-05T19:03:30Z
updated_at: 2026-06-05T19:34:06Z
assignee: null
---

## Background

Ticket orchestration now has:

- typed Ticket backend and tools;
- Ticket Intake and Orchestrator Routing workflows;
- fixed Ticket role config in `.yoi/ticket.config.toml`.

TUI and future CLI/orchestration UI should not construct role-specific Pod launch requests by hand. Before adding TUI Ticket role actions, introduce a reusable launcher layer that turns a fixed Ticket role plus context into a Pod spawn/run request.

This layer should keep the Profile/SystemInstruction boundary intact: the selected Profile owns durable role/system behavior, while the launcher creates the first committed user/task message for a concrete Ticket/action and includes the workflow reference as explicit model-visible input.

## Goal

Add a reusable Ticket role Pod launcher API that can be used by TUI and later CLI/orchestrator surfaces.

MVP should construct and optionally execute host-side Pod launches for fixed Ticket roles using `.yoi/ticket.config.toml`, selected Profile selector, workflow ref, and generated initial task prompt.

## Requirements

- Support fixed Ticket roles only:
  - `intake`
  - `orchestrator`
  - `coder`
  - `reviewer`
  - `investigator`
- Load `.yoi/ticket.config.toml` through `ticket::config::TicketConfig`.
- Use role `profile` selector as the child Pod profile selector.
- Use role `workflow` ref as a model-visible workflow invocation / workflow instruction in the first `Method::Run`.
- Generate a first committed user/task message from a typed launch context.
- Keep Profile-owned system instruction separate from the generated launch prompt.
- Do not add a role-level `system_instruction` override.
- Do not perform hidden context injection; everything dynamic must be sent as `Method::Run` input.
- Provide a launch planning API usable by TUI without depending on `pod` crate internals.
- Prefer placing the launcher in `client` if that keeps TUI dependency direction clean:
  - `tui -> client -> ticket/protocol/manifest`;
  - avoid `tui -> pod`.
- If implementing actual launch execution, use existing `client::spawn_pod` and `client::PodClient` / `protocol::Method::Run` where appropriate.
- Generate stable, testable Pod names or accept caller-provided names.
- Keep launch prompt text bounded and deterministic enough for tests.
- Include enough context for each role without over-scoping:
  - target Ticket id/slug;
  - user instruction or routing/action summary;
  - workflow slug;
  - optional intent packet;
  - optional worktree path / branch;
  - optional validation/report expectations.

## Launch prompt / workflow semantics

The first run should separate content like this:

- Profile supplies the system/role behavior.
- Workflow ref supplies the procedural flow to follow.
- Generated launch prompt supplies this specific Ticket/action context.

Prefer `Method::Run` with typed `Segment::WorkflowInvoke { slug }` plus `Segment::Text { content }` when practical. If a workflow segment is not viable in the immediate integration point, include the workflow slug explicitly in the generated text and document the limitation.

Configured `launch_prompt` refs may remain unresolved in the MVP if prompt-resource resolution is not available below `pod`; they should be exposed in the launch plan for future resolution and not silently treated as system instruction.

## Non-goals

- TUI action UI.
- Arbitrary role registry.
- Scheduler/lease/queue automation.
- Stateful workflow engine.
- Phase-specific tool gating.
- Role-level `system_instruction` support.
- Changing Profile resolution semantics.
- Changing `SpawnPod` tool semantics inside the `pod` crate.
- Implementing coder/reviewer worktree creation policy.
- Broad Pod registry/metadata redesign.

## Acceptance criteria

- A reusable launch plan/API exists for fixed Ticket roles.
- The launcher reads `.yoi/ticket.config.toml` defaults and configured role profile/workflow/launch_prompt refs.
- Profile selector selected for the role is available for spawn config.
- Dynamic task content is represented as first-run input, not hidden system/context injection.
- Workflow slug is included as a typed workflow segment or explicit model-visible instruction.
- TUI can consume the API without depending on `pod` internals.
- Tests cover:
  - default config role launch plan;
  - configured role profile/workflow/launch_prompt refs;
  - generated prompt content for at least intake/orchestrator/reviewer;
  - caller-provided Pod name;
  - missing/malformed Ticket config error surfacing;
  - no `system_instruction` handling.
- `cargo test -p client` or chosen crate tests pass.
- `cargo test -p ticket` passes if touched.
- `cargo check --workspace --all-targets`, `cargo fmt --check`, `git diff --check`, and `./tickets.sh doctor` pass.

## Follow-up tickets

- `tui-ticket-role-actions`: expose fixed Ticket role actions in TUI using this launcher.
- Prompt resolution follow-up: resolve `launch_prompt` refs into first-run content once a suitable prompt-resource API exists below/available to the launcher.
- Workflow-state follow-up: persist phase/state and commit phase prompts to history before model use.
