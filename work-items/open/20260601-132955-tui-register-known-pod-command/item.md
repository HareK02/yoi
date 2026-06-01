---
id: 20260601-132955-tui-register-known-pod-command
slug: tui-register-known-pod-command
title: TUI: add command to register a known Pod
status: open
kind: task
priority: P2
labels: [tui, pod, command, orchestration]
created_at: 2026-06-01T13:29:55Z
updated_at: 2026-06-01T16:19:25Z
assignee: null
legacy_ticket: null
---

## Background

When a user is attached to one running Pod, there is currently no user-facing way to tell that Pod about another existing Pod. As a result, the current Pod's `ListPods` tool surface only sees Pods that are already visible through its durable parent/child/current visibility state.

The desired `:` command is not primarily a UI attach/switch command. It should make another Pod known to the currently attached Pod, so that subsequent `ListPods` calls from that Pod include the registered target and existing Pod tools such as `RestorePod` can operate on that visible Pod where appropriate.

## Requirements

- Add a TUI command-mode entry point for registering another Pod as known/visible to the current Pod, tentatively `:know-pod <name>`, `:link-pod <name>`, or similar.
- The command should send an explicit Pod/runtime method to the currently attached Pod; it must not be a TUI-only local list mutation.
- The durable effect should live in Pod current-state metadata or an equivalent Pod-authoritative visibility record so it survives reconnect/restore where appropriate.
- `ListPods` from the current Pod should include the registered Pod after the command succeeds.
- The registered relationship must be distinguishable from spawned-child delegation. It must not imply delegated filesystem scope, parent ownership, child completion notifications, or child output cursor authority unless the existing runtime explicitly supports those semantics.
- The command should resolve the target by Pod name using existing Pod metadata/live registry visibility that is available to the controller/human, then record a safe reference for the current Pod.
- Handle at least:
  - target Pod is live;
  - target Pod is stopped but restorable;
  - target Pod name is unknown or ambiguous;
  - current TUI is not attached to a Pod;
  - current Pod is in a state where the registration method cannot be safely delivered.
- The current Pod's model conversation history must not be polluted merely because the user registers a known Pod. This is Pod metadata/control state, not a user message to the model.
- Provide clear command diagnostics/actionbar feedback.
- Add focused tests for command parsing, runtime method handling, metadata persistence/restore, and `ListPods` visibility behavior. If full live TUI/socket E2E is not feasible, document the manual validation path.

## Non-goals

- Do not implement arbitrary autonomous Pod-to-Pod messaging.
- Do not switch the TUI's active attachment as the primary behavior of this command.
- Do not replace the multi-Pod dashboard or Pod picker.
- Do not treat registered known Pods as spawned children.
- Do not make child completion notifications authoritative.
- Do not add delegated scope or reclaim behavior for known Pods unless a later design explicitly requires it.

## Acceptance criteria

- A user can invoke a documented `:` command from a single-Pod TUI to register another existing Pod as known to the current Pod.
- After successful registration, the current Pod's `ListPods` tool output includes the target Pod with a state/source label that does not misrepresent it as a spawned child.
- `RestorePod` or other visibility-scoped Pod tools can operate on the registered Pod according to the existing state-aware rules.
- The relationship is persisted in Pod-authoritative current state and survives ordinary TUI reconnect/Pod restore where the target remains visible/restorable.
- Failure modes produce clear diagnostics and do not modify history or create partial misleading visibility records.
- Tests cover parser/model/runtime visibility behavior, and implementation notes record any manual validation required for live TUI behavior.
- Documentation/help text for TUI commands and Pod visibility semantics is updated.
- Validation includes relevant TUI/client/pod-store/pod tests, `./tickets.sh doctor`, `git diff --check`, and `nix build .#yoi` unless explicitly deferred with rationale.
