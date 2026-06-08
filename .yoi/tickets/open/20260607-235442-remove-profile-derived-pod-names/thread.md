<!-- event: create author: LocalTicketBackend at: 2026-06-07T23:54:42Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: decision author: hare at: 2026-06-08T00:05:23Z -->

## Decision

## Decision update: remove `resume_by_pod_name` / `--profile-pod-name` split

While removing Profile-derived Pod names, also remove the confusing startup split that led to this boundary problem.

Current issue:

- `SpawnConfig::resume_by_pod_name` does not mean "inherit only the name while resuming"; it means "pass `--pod <pod_name>` to the child process so name-keyed restore/create happens".
- When `resume_by_pod_name = false`, `spawn_pod` may omit `--pod <pod_name>`, allowing profile resolution to derive Pod identity from profile slug/source.
- `--profile-pod-name` exists as a workaround for profile startup needing a Pod name without using `--pod`, but this preserves the false separation between profile selection and runtime identity.

Desired direction:

- `--pod <name>` is the runtime Pod identity for both restore and fresh create.
- `--profile <selector>` is only the Profile recipe selection.
- `--workspace <path>` / runtime workspace context supplies the workspace root.
- Profile slug/source must never supply `pod.name`.

Implementation requirements:

- Remove `resume_by_pod_name` from `SpawnConfig` or replace it with a clearer startup-mode enum only if a distinct mode is truly needed.
- Make spawn helpers pass explicit Pod identity for all fresh/restore paths instead of conditionally omitting `--pod`.
- Remove `--profile-pod-name` from the public/runtime argument surface if possible; otherwise mark it internal/deprecated with a follow-up to remove it.
- Support `--pod <name> --profile <selector>` as the normal way to start a named Pod with a selected Profile.
- If restore/create semantics need to be controlled, model that explicitly as startup policy rather than a boolean named `resume_by_pod_name`.
- Add regression coverage for the previous failure mode: project profile `project:companion` must not create Pod `companion` when the runtime requested workspace Pod name is `yoi` or another workspace basename.

---

<!-- event: intake_summary author: intake at: 2026-06-08T00:14:28Z -->

## Intake summary

Existing Ticket refined rather than duplicated. Request is clear enough for Orchestrator routing: remove every Profile-derived Pod identity path, make `--pod <name>`/runtime policy the sole Pod identity source while `--profile <selector>` remains recipe selection, remove or deprecate `--profile-pod-name`, update spawn/profile callers to pass explicit runtime names, and add regression tests proving project profile slugs such as `project:companion` do not create Pods named `companion`. Readiness: implementation_ready. Needs preflight: true because this touches profile/manifest boundaries, Pod identity/startup semantics, CLI surface, and runtime caller behavior. Risk flags: profile-boundary, manifest-boundary, pod-identity, cli-startup, backward-compatibility.

---

<!-- event: state_changed author: intake at: 2026-06-08T00:14:28Z from: intake to: ready reason: intake_ready field: workflow_state -->

## State changed

Intake judged the existing Ticket complete enough for Orchestrator routing. It should be queued by the user before implementation starts; Orchestrator should consider a preflight because profile/manifest and Pod startup identity boundaries are affected.

---

<!-- event: state_changed author: workspace-panel at: 2026-06-08T00:32:56Z from: ready to: queued reason: queued field: workflow_state -->

## State changed

Ticket queued for Orchestrator routing.


---

<!-- event: decision author: orchestrator at: 2026-06-08T00:33:13Z -->

## Decision

Routing decision: preflight_needed

Reason:
- The Ticket is clear and important, but it changes Profile/manifest identity boundaries, runtime Pod startup semantics, and CLI naming surface.
- Intake explicitly recorded `needs_preflight: true` because the work touches profile-boundary, manifest-boundary, pod-identity, CLI startup, and compatibility concerns.
- The thread contains a binding human direction to remove the `resume_by_pod_name` / `--profile-pod-name` split, but no preflight note yet maps that decision to concrete startup modes, caller changes, diagnostics, and tests.
- This Ticket is tightly coordinated with runtime workspace context and default workspace-basename naming Tickets; implementation should not start before the split of responsibilities is recorded.

Evidence checked:
- Ticket body: background, goal, requirements, and acceptance criteria.
- Thread: human decision update, intake summary, `ready -> queued` event.
- Workspace state: `develop` has Ticket-record edits for this and related queued Tickets; no matching branch or worktree exists for this slug.
- Visible Pods: Intake peers for this and related Tickets are live; no coder/reviewer implementation Pods are active.

Next action:
- Run `ticket-preflight-workflow` for this Ticket before implementation delegation.
- Preflight should produce a concise binding note covering: removal/replacement of `resume_by_pod_name`, normal `--pod <name> --profile <selector>` startup semantics, fate of `--profile-pod-name`, which caller paths must supply runtime Pod identity, diagnostics for forbidden Profile identity fields, regression tests for `project:companion`, and boundaries with `introduce-runtime-workspace-root-context` / `use-workspace-basename-for-default-pod-name`.
- Leave this Ticket queued for now; do not transition `queued -> inprogress`, create `.worktree/remove-profile-derived-pod-names`, or spawn coder/reviewer Pods until preflight records implementation readiness.

Escalate if:
- Removing `--profile-pod-name` requires a public compatibility policy beyond this Ticket.
- Fresh/create versus restore startup behavior needs a new explicit startup-mode model.
- Runtime workspace-root work must land first to provide the default naming source.

---

<!-- event: decision author: hare at: 2026-06-08T00:46:13Z -->

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
- The shared preflight decision now fixes the missing authority boundary: Profile selection is a recipe only and must not determine runtime Pod identity.
- This Ticket can be implemented as part of the coordinated runtime workspace / Pod identity cleanup bundle with `introduce-runtime-workspace-root-context` as the primary intent packet.

Evidence checked:
- Ticket body and thread, including the new shared boundary decision.
- Primary routing/IntentPacket recorded on `introduce-runtime-workspace-root-context`.
- Code map: `crates/manifest/src/profile.rs`, `crates/client/src/spawn.rs`, `crates/pod/src/entrypoint.rs`, `crates/tui/src/spawn.rs`, `crates/tui/src/multi_pod.rs`, and `crates/client/src/ticket_role.rs`.

Next action:
- Implement in the same coordinated branch as `introduce-runtime-workspace-root-context` and `use-workspace-basename-for-default-pod-name`.
- Before any implementation side effects, transition this Ticket and the bundle peers from `queued` to `inprogress` together.

Binding points for this Ticket:
- Remove Profile-derived Pod name fallback.
- Preserve explicit `--pod <name>` precedence.
- Treat `--profile <selector>` as recipe selection only.
- Remove or collapse `resume_by_pod_name` / `--profile-pod-name` split unless a real explicit startup-mode model is required.
- Add regression coverage proving `project:companion` does not imply Pod name `companion`.

Escalate if:
- Removing `--profile-pod-name` requires a public compatibility policy.
- Any startup path cannot provide runtime Pod identity without falling back to Profile slug/source.
- Implementation tries to preserve old Profile-derived names for compatibility rather than removing the design problem.

---

<!-- event: state_changed author: orchestrator at: 2026-06-08T00:57:20Z from: queued to: inprogress reason: orchestrator_acceptance field: workflow_state -->

## State changed

Accepted as part of the coordinated runtime workspace / Pod identity implementation bundle. This acceptance precedes worktree creation and coder/reviewer Pod spawning.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-08T01:53:53Z -->

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
