---
id: 20260607-220225-commit-intake-claims-after-launch-success
slug: commit-intake-claims-after-launch-success
title: Commit Intake ticket claims only after successful launch
status: open
kind: task
priority: P1
labels: [tui, panel, ticket, intake, bug]
workflow_state: intake
created_at: 2026-06-07T22:02:25Z
updated_at: 2026-06-07T22:02:25Z
assignee: null
legacy_ticket: null
---

## Background

Panel Clarify / existing-Ticket Intake launch currently writes a local Ticket claim before the Intake Pod launch has actually succeeded. If the Pod launch later fails, the claim can remain and make the Ticket look claimed by a ghost/stale Intake Pod.

Observed behavior:

```text
Ticket <id> has a stale local Intake claim for <pod_name>; explicit reclaim/diagnostic is required before starting a replacement.
```

This is especially visible when the first Clarify launch for an unclaimed Ticket fails because of profile/scope or other launch errors. The Ticket was never successfully claimed by a real Intake Pod, but the local registry blocks a replacement as if it had been.

## Goal

Ensure local Intake Ticket claims represent confirmed launched/restorable Intake Pods, not speculative launch attempts.

## Requirements

- Do not write a durable local Ticket claim until Intake Pod launch has succeeded with sufficient acceptance evidence.
  - At minimum, spawn/connect/initial `Method::Run` acceptance should complete before the claim is committed.
- If the implementation keeps the current pre-claim structure, every launch failure path must reliably roll back the just-created claim.
  - Prefer committing the claim after successful launch over rollback-heavy pre-claiming.
- Failed first launch for an unclaimed Ticket must not leave a stale local claim.
- Successful existing-Ticket Intake launch must still write the local claim and role-session record.
- Duplicate launch prevention during an in-flight launch should use transient/in-memory pending state or another non-durable mechanism, not a durable claim that can outlive a failed launch.
- Existing stale claims should remain diagnostic/manual-reclaim territory unless the implementation can safely distinguish claims created by the current failed attempt.
- Keep the behavior that live/restorable existing claims prevent starting a second Intake Pod.
- Preserve Ticket Intake handoff peer registration behavior where possible.
- Do not change Ticket workflow-state semantics, Orchestrator lifecycle, or Companion routing.

## Acceptance criteria

- A simulated Intake launch failure for a previously unclaimed Ticket leaves no local claim file/record for that Ticket.
- A simulated successful Intake launch writes exactly one claim for the launched Pod.
- Retrying after a failed launch is not blocked by a ghost/stale claim from the failed attempt.
- Existing live/restorable claims still block duplicate Intake launch and direct the user to the existing Pod.
- Tests cover failure before spawn, failure after spawn/connect or initial Run rejection/timeout if practical, and successful launch.
- Focused TUI/registry tests pass.
- `cargo test -p tui ... --lib`, `cargo fmt --check`, `git diff --check`, and `target/debug/yoi ticket doctor` pass.
