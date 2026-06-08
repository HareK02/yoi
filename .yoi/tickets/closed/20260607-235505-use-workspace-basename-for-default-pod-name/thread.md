<!-- event: create author: LocalTicketBackend at: 2026-06-07T23:55:05Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: intake_summary author: intake at: 2026-06-08T00:14:37Z -->

## Intake summary

Existing Ticket refined enough for Orchestrator routing. The request is to remove product-name-specific default Pod identity (`"yoi"`/`DEFAULT_POD_NAME`) and unify fresh default Pod naming on the sanitized workspace/current-directory basename, while preserving explicit `--pod` precedence, keeping Orchestrator/role Pods distinct and explicit, and ensuring Profile selection never determines Pod identity. Readiness: implementation_ready. Needs preflight: true, because this touches Pod identity, CLI/TUI/Panel launch behavior, Profile/runtime boundary, and Pod metadata/restore expectations. Risk flags: pod-identity, profile-boundary, cli-tui-panel-consistency, persistence. Related work: coordinate with `remove-profile-derived-pod-names`; keep `add-pod-archive-fresh-start-path` out of scope.

---

<!-- event: state_changed author: intake at: 2026-06-08T00:14:37Z from: intake to: ready reason: intake_ready field: workflow_state -->

## State changed

Intake completed for existing Ticket; ready for human queueing and Orchestrator routing. Implementation must not start until the panel/user moves ready -> queued and Orchestrator records queued -> inprogress.

---

<!-- event: state_changed author: workspace-panel at: 2026-06-08T00:33:01Z from: ready to: queued reason: queued field: workflow_state -->

## State changed

Ticket queued for Orchestrator routing.


---

<!-- event: decision author: orchestrator at: 2026-06-08T00:33:29Z -->

## Decision

Routing decision: preflight_needed

Reason:
- The Ticket is clear enough to route, but it changes runtime Pod identity defaults across CLI, TUI, Panel, and helper launch paths.
- Intake explicitly recorded `needs_preflight: true` because this touches Pod identity, Profile/runtime separation, CLI/TUI/Panel consistency, and persistence/restore expectations.
- This Ticket must be coordinated with `remove-profile-derived-pod-names` and `introduce-runtime-workspace-root-context` so the default workspace basename policy, explicit `--pod` precedence, Profile non-influence, and Orchestrator/role Pod naming split are binding before coder delegation.
- No preflight/design note is present yet that fixes the exact default naming policy and the caller/test boundaries.

Evidence checked:
- Ticket body: background, goal, requirements, non-goals, and acceptance criteria.
- Thread: intake summary and `ready -> queued` event.
- Workspace state: `develop` has Ticket-record edits for this and related queued Tickets; no matching branch or worktree exists for this slug.
- Visible Pods: Intake peers for this and related Tickets are live; no coder/reviewer implementation Pods are active.

Next action:
- Run `ticket-preflight-workflow` for this Ticket before implementation delegation.
- Preflight should record: sanitized workspace/current-directory basename rules, generic fallback behavior, explicit `--pod` precedence, Orchestrator/role Pod naming invariants, Panel Companion expectations, relationship to Profile-derived-name removal, non-goals around existing metadata/archive behavior, and test/validation targets using a non-`yoi` workspace.
- Leave this Ticket queued for now; do not transition `queued -> inprogress`, create `.worktree/use-workspace-basename-for-default-pod-name`, or spawn coder/reviewer Pods until preflight records implementation readiness.

Escalate if:
- Default naming requires a metadata migration for existing Pods.
- Workspace-root context must be introduced first to compute defaults consistently.
- Any caller still requires Profile slug/source as an identity fallback.

---

<!-- event: decision author: hare at: 2026-06-08T00:46:34Z -->

## Decision

## Shared Pod identity / workspace boundary decision

This decision is shared across these related Tickets:

- `introduce-runtime-workspace-root-context`
- `remove-profile-derived-pod-names`
- `use-workspace-basename-for-default-pod-name`

The three Tickets should be treated as a coordinated cleanup bundle, not as three unrelated design unknowns requiring separate heavy preflight. This decision supplies the binding boundary needed for Orchestrator routing; remaining uncertainty should be treated as coder implementation investigation unless it hits an escalation condition below.

Binding decisions:

- `--workspace <path>` represents the runtime workspace root.
- `--pod <name>` represents the runtime Pod identity for both restore and fresh create.
- `--profile <selector>` represents reusable Profile recipe selection only.
- Lua Profile fields, Profile slug, Profile source, and registry entry names must not define or imply `pod.name`.
- Default Pod name derives from the runtime workspace basename, not from product name, Profile slug/source, `.yoi` project-record marker, or memory root detection.
- This repository's normal default Pod name is `yoi` only because the workspace directory basename is `yoi`.
- Workspace Orchestrator remains a distinct runtime identity such as `<workspace>-orchestrator`.
- Ticket role / task Pods should continue to use explicit role/task-derived names supplied by the launcher/orchestrator, not profile fallback names.
- `.yoi` project records root, `.yoi/memory` memory root, and runtime workspace root are separate concepts.
- Memory root behavior from `memory-root-uses-yoi-memory-marker` must be preserved: explicit `memory.workspace_root` wins; otherwise nearest `.yoi/memory`; `.yoi` alone is not a memory root marker.
- `resume_by_pod_name` / `--profile-pod-name` are cleanup targets. Prefer collapsing startup around explicit `--pod <name> --profile <selector>` plus runtime workspace context. If an explicit startup-mode enum is needed, it should model restore/create policy directly rather than hide it behind a boolean named `resume_by_pod_name`.
- Existing Pod metadata migration/renaming and archive/fresh-start behavior are out of scope for this implementation bundle unless explicitly required by tests; `add-pod-archive-fresh-start-path` covers archive/fresh-start UX.

Implementation latitude:

- The coder may discover exact call sites, structure names, helper placement, and test organization during implementation.
- The coder may split changes across the three Tickets if needed, but should preserve the above boundaries and avoid adding compatibility layers just to preserve profile-derived identity.
- Tests should include non-`yoi` workspace names so dogfooding does not hide hardcoded/default-name bugs.

Escalate if:

- Removing `--profile-pod-name` requires a public compatibility policy beyond internal cleanup.
- Existing Pod metadata needs automatic migration or destructive modification.
- A caller genuinely cannot supply runtime workspace root or Pod identity without reintroducing profile-derived naming.
- The implementation would make `.yoi` alone a memory root marker again.
- The implementation would weaken explicit `--pod` precedence or make Profile selection influence Pod identity.

With this decision recorded, this Ticket can proceed as implementation-ready if Orchestrator can write a bounded IntentPacket from the Ticket body and this shared decision. A separate heavy preflight is not required unless one of the escalation conditions is reached.

---

<!-- event: decision author: orchestrator at: 2026-06-08T00:52:41Z -->

## Decision

Routing decision: implementation_ready

Reason:
- The shared preflight decision now fixes the default identity policy: default Pod identity derives from runtime workspace basename, while Profile selector/source never influences identity and explicit `--pod` always wins.
- This Ticket can be implemented as part of the coordinated runtime workspace / Pod identity cleanup bundle with `introduce-runtime-workspace-root-context` as the primary intent packet.

Evidence checked:
- Ticket body and thread, including the new shared boundary decision.
- Primary routing/IntentPacket recorded on `introduce-runtime-workspace-root-context`.
- Code map: hardcoded/default name paths in `crates/manifest/src/profile.rs`, TUI/Panel companion lifecycle, client spawn helper, Pod entrypoint, and Ticket role launch paths.

Next action:
- Implement in the same coordinated branch as `introduce-runtime-workspace-root-context` and `remove-profile-derived-pod-names`.
- Before any implementation side effects, transition this Ticket and the bundle peers from `queued` to `inprogress` together.

Binding points for this Ticket:
- Fresh default Pod name is sanitized runtime workspace/current-directory basename.
- Use a generic fallback such as `pod` only when no useful basename exists.
- This repository defaults to `yoi` only because the workspace basename is `yoi`.
- Panel Companion uses the workspace basename Pod, while Orchestrator remains distinct such as `<workspace>-orchestrator`.
- Ticket role Pods keep explicit role/task-derived names.
- Tests must include a non-`yoi` workspace and explicit `--pod` precedence.

Escalate if:
- Existing Pod metadata would need automatic rename/migration.
- Workspace-root context is insufficient to compute defaults consistently.
- Any caller still requires Profile slug/source as an identity fallback.

---

<!-- event: state_changed author: orchestrator at: 2026-06-08T00:57:20Z from: queued to: inprogress reason: orchestrator_acceptance field: workflow_state -->

## State changed

Accepted as part of the coordinated runtime workspace / Pod identity implementation bundle. This acceptance precedes worktree creation and coder/reviewer Pod spawning.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-08T01:53:54Z -->

## Implementation report

Merge-ready dossier pointer for coordinated bundle.

This Ticket was implemented and reviewed as part of the `runtime-workspace-context` branch/worktree bundle. The full merge-ready dossier is recorded on primary Ticket `introduce-runtime-workspace-root-context`.

Relevant branch/worktree:
- Branch: `runtime-workspace-context`
- Worktree: `.worktree/runtime-workspace-context`
- Commits:
  - `b6af761 runtime: separate workspace pod and profile identity`
  - `15f54df runtime: use pod flag for session identity`

Status:
- Reviewer re-review verdict: approve.
- No remaining reviewer blockers for this Ticket.
- Merge authority is still required; no merge, close, final approval, or cleanup has occurred.

---

<!-- event: review author: orchestrator at: 2026-06-08T01:59:52Z status: approve -->

## Review: approve

Final merge-completion approval after coordinated bundle merge and validation.

Evidence:
- Implemented as part of branch `runtime-workspace-context`.
- Reviewer approved after fix-loop.
- Post-merge validation passed, including focused profile/spawn/entrypoint/CLI/TUI tests, `cargo check -q`, `cargo fmt --check`, `git diff --check`, ticket doctor, and `nix build .#yoi`.
- Cleanup completed for the merged runtime-workspace branch/worktree and coder/reviewer Pods.

---

<!-- event: state_changed author: orchestrator at: 2026-06-08T01:59:59Z from: inprogress to: done reason: merged_and_validated field: workflow_state -->

## State changed

Completed as part of the merged runtime workspace identity bundle; post-merge validation passed and cleanup completed.

---

<!-- event: close author: hare at: 2026-06-08T02:00:18Z status: closed -->

## Closed

Completed as part of the coordinated runtime workspace / Pod identity bundle.

Summary:
- Replaced product-name-specific default Pod identity with runtime workspace basename-based default naming.
- Preserved explicit `--pod` precedence.
- Kept workspace Orchestrator and Ticket role/task Pod names explicit and distinct from the default Companion/workspace Pod.
- Added/validated non-`yoi` workspace/default naming coverage so the dogfooding repository name no longer masks hardcoded defaults.

Merged branch:
- `runtime-workspace-context` via merge commit `b7a533f`.

Validation and cleanup:
- Post-merge focused tests, `cargo check -q`, `cargo fmt --check`, `git diff --check`, ticket doctor, and `nix build .#yoi` passed.
- Runtime-workspace coder/reviewer Pods, worktree, and branch were cleaned up.

---
