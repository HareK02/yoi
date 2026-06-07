<!-- event: create author: LocalTicketBackend at: 2026-06-07T01:21:31Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: plan author: hare at: 2026-06-07T01:42:08Z -->

## Plan

## Preflight / implementation intent

Classification: implementation-ready with coordination risk.

Intent:
- Add a local, workspace-scoped role session registry and Ticket claim index for `yoi panel` orchestration state.
- Store registry data under the configured user data directory, not under git-tracked `.yoi/tickets` files and not under the runtime directory.
- Model `Ticket -> at most one active local Pod claim` while allowing `Pod/session -> 0..N related Tickets`.
- Represent pre-Ticket Intake sessions even before a Ticket exists.
- Join Ticket state, local registry data, and Pod metadata in panel state/actions where practical, without introducing automatic polling/spawn.

Requirements:
- Define a stable workspace key/path for registry storage under `manifest::paths::data_dir()` or an equivalent existing user-data boundary.
- Keep local Pod names, socket paths, runtime status, and local claim state out of git-tracked Ticket frontmatter/thread files.
- Make claim creation race-aware: when starting Intake for an existing Ticket, claim before spawn/restore; if a claim exists, do not start a second Pod silently.
- Treat stale claims explicitly with diagnostics/reclaim affordance rather than auto-replacing them.
- Intake is not 1:1 with Ticket: support no related Ticket and multiple related Tickets in the local model.
- Preserve the current interim access path where `ticket-*` Pods are visible in the panel.

Code map:
- `crates/manifest/src/paths.rs`: existing data/config/runtime directory boundary; registry should use durable user data, not runtime.
- `crates/tui/src/multi_pod.rs`: Intake launch path (`prepare_intake_launch` / `finish_intake_launch`), panel snapshot construction, selected actions.
- `crates/tui/src/workspace_panel.rs`: Ticket rows, related Pod display, `workflow_state = intake` row behavior, Panel view model.
- `crates/client/src/ticket_role.rs`: Ticket role launch context and Pod naming; useful integration point but should not write git-tracked Ticket assignment.
- Existing Pod list/status model is in `crates/tui/src/multi_pod.rs` / `workspace_panel.rs`; use it for liveness/restorability rather than persisting live status as authority.

Boundaries / non-goals:
- Do not store local Pod assignment in Ticket metadata/thread.
- Do not assume Intake Pod and Ticket are 1:1.
- Do not add polling that auto-starts Intake for `workflow_state = intake` Tickets.
- Do not replace Orchestrator scheduling.
- Do not implement the real Companion Pod lifecycle here.
- Do not remove selected-Pod direct-send here; `workspace-panel-remove-direct-pod-send` owns that and is being implemented in parallel.

Coordination note:
- `workspace-panel-remove-direct-pod-send` is active in branch `work/workspace-panel-remove-direct-pod-send` and may touch `multi_pod.rs` / `workspace_panel.rs`. Keep this registry implementation as additive/localized as possible and report any likely merge conflicts.

Validation:
- Add focused unit tests for registry serialization/path behavior, one-active-claim-per-Ticket invariant, and non-1:1 Intake/Ticket relation.
- Run focused `cargo test -p tui workspace_panel --lib` and relevant new registry tests; run `cargo test -p tui multi_pod --lib` if integration touches panel runtime paths; run formatting/checks appropriate for the diff.

---

<!-- event: intake_summary author: INSOMNIA at: 2026-06-07T01:42:13Z -->

## Intake summary

Implementation intent is clear: add a user-data workspace-scoped local role/session registry and Ticket claim index, enforce one active local Pod claim per Ticket, allow one Intake session to relate to multiple Tickets or none, and keep local Pod assignment out of git-tracked Ticket records.

---

<!-- event: state_changed author: INSOMNIA at: 2026-06-07T01:42:13Z from: intake to: ready reason: intake_ready field: workflow_state -->

## State changed

Ticket intake complete; workflow_state intake -> ready.


---

<!-- event: implementation_report author: hare at: 2026-06-07T01:42:51Z -->

## Implementation report

## Delegation

Implementation delegated to child Pod `panel-role-session-registry-coder-20260607` in worktree `.worktree/workspace-panel-local-role-session-registry` on branch `work/workspace-panel-local-role-session-registry`.

Scope:
- Add a durable user-data workspace-scoped local role session registry and Ticket claim index.
- Enforce one active local Pod claim per Ticket while allowing a role session/Intake Pod to relate to zero or more Tickets.
- Keep local Pod assignment out of git-tracked Ticket metadata/thread.
- Preserve no-polling semantics and the interim `ticket-*` Pod visibility path.

Coordination note: `workspace-panel-remove-direct-pod-send` is active in parallel and may touch `multi_pod.rs` / `workspace_panel.rs`; the child was instructed to keep the registry implementation additive/localized and report likely conflicts.

The child should commit implementation work in the child worktree and report diff/tests. Merge, review, Ticket closure, and cleanup remain with the parent/human workflow.

---

<!-- event: implementation_report author: hare at: 2026-06-07T02:04:23Z -->

## Implementation report

## Review delegation

External review delegated to `panel-role-session-registry-reviewer-20260607` with read-only scope over the repository.

Review focus:
- Validate the local user-data registry and Ticket claim model against the ticket intent.
- Check one-active-claim-per-Ticket behavior, non-1:1 Intake/Ticket support, claim-before-spawn behavior, stale claim handling, and workspace-key/path choices.
- Confirm local Pod assignment is not written to git-tracked Ticket metadata/thread.
- Explicitly review semantic/merge conflicts with current `develop`, because `workspace-panel-remove-direct-pod-send` has already landed and touched `multi_pod.rs` / `workspace_panel.rs` after this implementation branch was created.

The reviewer is instructed not to modify files and to return approve/request-changes guidance with evidence and merge-readiness notes.

---
