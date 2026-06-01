---
id: 20260601-132955-tui-pod-connect-command
slug: tui-pod-connect-command
title: TUI: add command for connecting to another Pod
status: open
kind: task
priority: P2
labels: [tui, pod, command, orchestration]
created_at: 2026-06-01T13:29:55Z
updated_at: 2026-06-01T13:29:55Z
assignee: null
legacy_ticket: null
---

## Background

When a user is attached to one running Pod in the TUI, there is currently no small command-mode affordance for making another Pod reachable/contactable from that session. Existing Pod discovery/restore mechanisms exist at other layers (`ListPods`/`RestorePod`, resume picker, multi-Pod dashboard), but the single-Pod TUI command surface only exposes local commands such as `:help`, `:compact`, and `:rewind`.

Add a `:` command that lets a user connect to another visible Pod from within the running TUI. The exact UX can be refined during implementation, but it should be command-mode based and should not make background notifications or transient socket state into durable authority.

## Requirements

- Add a TUI command-mode entry point for connecting to another Pod, tentatively `:pod <name>` or `:connect <pod-name>`.
- The command should use the existing Pod visibility/attach/restore authority rather than inventing a separate registry.
- It should work from a single-Pod TUI session without requiring the user to exit to a dashboard.
- It should handle at least:
  - target Pod is live and attachable;
  - target Pod is stopped but restorable;
  - target Pod is not visible;
  - current Pod is busy/streaming and command execution would be unsafe.
- The current Pod's durable history must not be rewritten or polluted merely because the user connects to another Pod.
- The UX should make it clear whether the command switches the active TUI attachment, opens a temporary contact/send operation, or enters a small Pod picker. Choose one behavior and document the rationale.
- Avoid duplicating old tool names or semantics; prefer current `ListPods` / `RestorePod`-style state concepts.
- Add focused tests for command parsing/availability and any pure model logic. If runtime socket/TUI E2E is not feasible, record the manual validation path.

## Non-goals

- Do not implement arbitrary Pod-to-Pod autonomous messaging unless explicitly designed.
- Do not replace the multi-Pod dashboard.
- Do not make child completion notifications authoritative.
- Do not add a new persistent relationship graph between Pods unless required by a later design.

## Acceptance criteria

- A user can invoke a documented `:` command from a single-Pod TUI to connect to another visible Pod by name or through a small selection flow.
- The command uses existing Pod attach/restore/discovery mechanisms and preserves Pod/session authority boundaries.
- Failure modes produce clear actionbar/command diagnostics.
- Tests cover parser/availability/model behavior, and implementation notes record any manual validation required for live attach behavior.
- Documentation/help text for TUI commands is updated.
- Validation includes relevant TUI/client tests, `./tickets.sh doctor`, `git diff --check`, and `nix build .#yoi` unless explicitly deferred with rationale.
