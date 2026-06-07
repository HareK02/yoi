<!-- event: create author: LocalTicketBackend at: 2026-06-07T03:35:36Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: plan author: hare at: 2026-06-07T03:47:42Z -->

## Plan

## Preflight / implementation intent

Classification: implementation-ready and safe to run in parallel with `ticket-init-role-profile-scaffold`.

Parallelism note:
- `ticket-init-role-profile-scaffold` is expected to touch `crates/yoi/src/ticket_cli.rs`, `crates/ticket/src/config.rs`, and maybe role-launch tests.
- This ticket should primarily touch `crates/pod/src/feature/builtin/ticket.rs`, `crates/ticket/src/tool.rs`, feature registration tests, and possibly role/profile integration tests.
- Coordinate carefully if `ticket/src/config.rs` is touched, but the expected overlap is low.

Current code map / discovery:
- A built-in Ticket feature already exists at `crates/pod/src/feature/builtin/ticket.rs` with feature id `builtin:ticket`.
- It currently declares/registers all `ticket::tool::TICKET_TOOL_NAMES` through the feature registry and requires `HostAuthority::TicketBackend`.
- `ticket::tool::TICKET_TOOL_NAMES` already includes lifecycle tools such as `TicketIntakeReady` and `TicketWorkflowState`.
- The missing piece is not creating Ticket tools from nothing; it is shaping this existing Ticket-domain feature into an explicit lifecycle capability with authority subsets/grants suitable for roles such as read-only Companion vs mutating Orchestrator/Intake/coder/reviewer.

Intent:
- Keep this as a Ticket-domain feature, not an Orchestrator feature.
- Extend/refine the existing built-in Ticket feature so it can expose read-only vs mutating/lifecycle tool subsets according to profile/policy/grant selection.
- Preserve backend/tool transition enforcement; prompts can specify sequencing, but invalid workflow transitions must still fail in typed Ticket backend/tool paths.

Requirements for first implementation:
- Do not duplicate Ticket tools outside the feature system.
- Do not create an Orchestrator-specific feature id.
- Provide a clear API/config surface for Ticket feature authority level or tool subset. Suggested initial shape: a `TicketFeatureAccess` / `TicketToolSet` enum or builder with at least:
  - read-only/status: `TicketList`, `TicketShow`, maybe `TicketDoctor` if considered read-only diagnostic;
  - lifecycle/mutating: comment/review/intake-ready/workflow-state/status/close/create according to the existing tool set.
- Feature descriptor should declare only tools that the configured feature instance may register, and installation should register only that subset.
- Tests should prove read-only installation does not expose mutating tools, while lifecycle/mutating installation exposes the needed lifecycle tools.
- Tool descriptions for `TicketWorkflowState` should communicate the queued acceptance sequencing contract: implementation side effects should happen only after `queued -> inprogress` acceptance.
- Preserve existing `builtin:ticket` behavior where needed, or provide an intentional default that does not silently grant broad mutation to roles that should be read-only. If compatibility choices are ambiguous, prefer a small explicit API and report the integration follow-up.

Non-goals:
- Do not implement full Orchestrator queue automation.
- Do not implement workflow state-machine dynamic tool gating.
- Do not implement `StartTicketWork` / `AcceptQueuedTicket` unless it is a trivial wrapper; it is acceptable to leave composite orchestration as follow-up.
- Do not add Companion Bash or broad write authority.
- Do not alter local role session registry semantics.

Validation:
- Add/update focused tests in `pod::feature::builtin::ticket` and/or `ticket::tool`.
- Run at minimum `cargo test -p pod ticket --lib` and relevant `cargo test -p ticket tool --lib` if ticket tool partitions are added.
- Run `cargo fmt --check` and `git diff --check`; broader `cargo check --workspace --all-targets` if feature API surfaces change broadly.

---

<!-- event: intake_summary author: INSOMNIA at: 2026-06-07T03:47:55Z -->

## Intake summary

Implementation-ready: refine the existing built-in Ticket-domain feature (`builtin:ticket`) into a lifecycle capability with explicit read-only vs mutating/lifecycle tool subsets. This can run in parallel with ticket init scaffold because expected code overlap is low.

---

<!-- event: state_changed author: INSOMNIA at: 2026-06-07T03:47:55Z from: intake to: ready reason: intake_ready field: workflow_state -->

## State changed

Ticket intake complete; workflow_state intake -> ready.


---

<!-- event: implementation_report author: hare at: 2026-06-07T03:48:50Z -->

## Implementation report

## Delegation

Implementation delegated to child Pod `ticket-lifecycle-feature-coder-20260607` in worktree `.worktree/ticket-lifecycle-pod-feature` on branch `work/ticket-lifecycle-pod-feature`.

Scope:
- Refine the existing built-in Ticket-domain feature (`builtin:ticket`) rather than creating an Orchestrator-specific feature.
- Add explicit read-only/status vs mutating/lifecycle tool subset support.
- Keep Ticket transition enforcement in typed backend/tool paths.
- Do not implement full Orchestrator automation or composite `StartTicketWork` in this ticket.

Parallel note:
- This is running alongside `ticket-init-role-profile-scaffold`; expected code overlap is low. The child was instructed to keep changes localized and report any conflict risk.

The child should commit implementation work in the child worktree and report diff/tests. Merge, review, Ticket closure, and cleanup remain with the parent/human workflow.

---
