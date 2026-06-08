<!-- event: create author: LocalTicketBackend at: 2026-06-08T00:00:47Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: decision author: hare at: 2026-06-08T00:05:33Z -->

## Decision

## Coordination note: workspace context should pair with explicit Pod identity

The runtime workspace context cleanup should align with `remove-profile-derived-pod-names` by making startup boundaries explicit:

- `--workspace <path>`: runtime workspace root.
- `--pod <name>`: runtime Pod identity for restore or fresh create.
- `--profile <selector>`: reusable Profile recipe.

Avoid preserving the old split where profile startup uses `--profile-pod-name` and non-profile startup uses `--pod`. That split allowed `SpawnConfig::resume_by_pod_name = false` to omit the runtime identity and let profile slug/source derive `pod.name`.

The intended end state is that every Pod startup path has an explicit runtime workspace and explicit Pod identity before profile resolution runs. Profile resolution should consume those runtime values, not invent them.

---

<!-- event: intake_summary author: intake at: 2026-06-08T00:13:41Z -->

## Intake summary

Existing Ticket refined and duplicate check completed. Scope is to introduce a single explicit runtime workspace_root decided at startup and threaded through profile resolution, Pod startup/identity boundaries, Panel lifecycle, Ticket/project record discovery, and memory resolution while preserving separation from Profile-authored config, Pod identity, project records, and memory root. Readiness: implementation_ready for orchestration, but needs_preflight: true because it crosses CLI/profile/Pod metadata/persistence/panel/memory boundaries. Risk flags: workspace-context, profile-boundary, pod-identity, persistence, panel, memory-root, cli. Related Tickets to coordinate: remove-profile-derived-pod-names, use-workspace-basename-for-default-pod-name, add-pod-archive-fresh-start-path, memory-root-uses-yoi-memory-marker.

---

<!-- event: state_changed author: intake at: 2026-06-08T00:13:41Z from: intake to: ready reason: intake_ready field: workflow_state -->

## State changed

Intake considers this Ticket ready for Orchestrator routing. Queueing remains a separate user/panel action; implementation must not start until queued and accepted by the Orchestrator.

---

<!-- event: state_changed author: workspace-panel at: 2026-06-08T00:14:09Z from: ready to: queued reason: queued field: workflow_state -->

## State changed

Ticket queued for Orchestrator routing.


---

<!-- event: decision author: orchestrator at: 2026-06-08T00:14:32Z -->

## Decision

Routing decision: preflight_needed

Reason:
- The Ticket is implementation-oriented but crosses CLI startup, Profile resolution, Pod identity/startup, Pod metadata/session persistence, Panel lifecycle, Ticket/project record discovery, and memory-root resolution boundaries.
- Intake explicitly recorded `needs_preflight: true` and risk flags for workspace context, profile boundary, pod identity, persistence, panel, memory-root, and CLI concerns.
- The Ticket depends on coordination with related Tickets (`remove-profile-derived-pod-names`, `use-workspace-basename-for-default-pod-name`, `add-pod-archive-fresh-start-path`, and the completed memory-root marker behavior), so binding boundaries must be written before coder delegation.
- No preflight/design note is present yet that fixes which structures carry `workspace_root`, how restore/debug metadata is persisted, how `--workspace` interacts with `--pod`/`--profile`, and what is explicitly out of scope for the related naming/archive tickets.

Evidence checked:
- Ticket body: background, runtime workspace context requirements, boundary rules, CLI/process requirements, tests/validation, related Tickets, non-goals, and acceptance criteria.
- Thread: explicit human coordination note on pairing workspace context with explicit Pod identity, intake summary, `ready -> queued` event.
- Workspace state: `develop` has Ticket-record edits for this and another queued Ticket; no matching branch or worktree exists for this slug.
- Visible Pods: Intake peers are live for this and related naming Tickets; no coder/reviewer implementation Pods are active.

Next action:
- Run `ticket-preflight-workflow` for this Ticket before implementation delegation.
- Preflight should produce a concise binding design note covering: the runtime launch context type and ownership, startup call paths that must receive it, metadata/session persistence expectations, CLI surface (`--workspace`, `--pod`, `--profile`) boundaries, Panel/Ticket role launcher propagation, memory/project-root separation, related-ticket split, and focused validation plan.
- Leave this Ticket queued for now; do not transition `queued -> inprogress`, create `.worktree/introduce-runtime-workspace-root-context`, or spawn coder/reviewer Pods until preflight records implementation readiness.

Escalate if:
- A single `workspace_root` requires storage migration or compatibility policy for existing Pod metadata.
- The CLI boundary requires removing/renaming public flags beyond this Ticket's intended scope.
- Coordination with profile-derived naming/default Pod naming cannot be cleanly separated into the related Tickets.

---

<!-- event: decision author: hare at: 2026-06-08T00:45:54Z -->

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

<!-- event: decision author: orchestrator at: 2026-06-08T00:52:23Z -->

## Decision

Routing decision: implementation_ready

Reason:
- The shared human decision recorded on all three related Tickets supplies the missing preflight boundary: `--workspace` is runtime workspace root, `--pod` is runtime Pod identity, `--profile` is recipe selection only, and Profile slug/source/registry names must never imply `pod.name`.
- Remaining uncertainty is now bounded implementation investigation: exact call sites, type placement, helper names, and test organization.
- Current code map confirms the relevant surfaces are narrow enough for a coder IntentPacket: `crates/manifest/src/profile.rs` still derives Pod names from Profile source/slug and hardcoded `DEFAULT_POD_NAME`; `crates/client/src/spawn.rs`, `crates/tui/src/spawn.rs`, `crates/tui/src/multi_pod.rs`, `crates/pod/src/entrypoint.rs`, and `crates/client/src/ticket_role.rs` carry the spawn/profile identity split; Panel already has workspace companion/orchestrator name helpers that should be preserved/consumed.

Evidence checked:
- Ticket body and thread, including the new shared boundary decision.
- Related Tickets: `remove-profile-derived-pod-names` and `use-workspace-basename-for-default-pod-name` now contain the same binding decision.
- Workspace state: only Ticket/project-record files are dirty; no implementation branch/worktree exists yet.
- Code map searches for `DEFAULT_POD_NAME`, `derive_pod_name`, `resume_by_pod_name`, `profile_pod_name`, `workspace_root`, `SpawnConfig`, Panel, and Ticket role launch paths.
- Ticket doctor: 0 errors; existing warnings are unrelated legacy closed-Ticket diagnostics.

IntentPacket:

Intent:
- Implement the coordinated runtime workspace / Pod identity cleanup bundle across this Ticket plus `remove-profile-derived-pod-names` and `use-workspace-basename-for-default-pod-name`.
- Make startup identity explicit: runtime workspace root, runtime Pod name, and Profile selector are separate values.

Binding decisions / invariants:
- `--workspace <path>` represents runtime workspace root.
- `--pod <name>` represents runtime Pod identity for both restore and fresh create.
- `--profile <selector>` represents reusable Profile recipe selection only.
- Profile fields, Profile slug/source, and registry entry names must not define or imply `pod.name`.
- Default Pod name derives from the runtime workspace basename; this repo defaults to `yoi` only because the directory basename is `yoi`.
- Workspace Orchestrator remains distinct, e.g. `<workspace>-orchestrator`.
- Ticket role/task Pods keep explicit launcher/orchestrator-provided names.
- `.yoi` project records root, `.yoi/memory` memory root, and runtime workspace root stay separate.
- Preserve memory-root behavior: explicit `memory.workspace_root` wins; otherwise nearest `.yoi/memory`; `.yoi` alone is not a memory root marker.
- Existing Pod metadata migration/renaming and archive/fresh-start UX are out of scope unless tests force a minimal non-destructive adjustment.

Requirements / acceptance criteria:
- Introduce/pass a single explicit runtime `workspace_root` through startup/profile/Panel/Pod runtime paths where needed.
- Add or wire a clear CLI/process boundary for `--workspace`, `--pod`, and `--profile`.
- Remove Profile-derived Pod name fallback in `crates/manifest/src/profile.rs`.
- Remove or collapse the `resume_by_pod_name` / `--profile-pod-name` split unless a real explicit startup-mode enum is necessary.
- Replace product-name-specific default identity with sanitized workspace basename and generic fallback only when no useful basename exists.
- Keep explicit `--pod` precedence.
- Preserve Panel Companion and Orchestrator lifecycle semantics while making Companion use workspace basename and Orchestrator use a distinct explicit name.
- Add regression tests using non-`yoi` workspace names and Profile selectors such as `project:companion`.

Implementation latitude:
- Coder may choose exact type names, helper placement, call-site order, and test module organization.
- Coder may implement the three Tickets as one coordinated branch if the diff remains coherent and each Ticket's acceptance criteria are addressed.
- Coder may retain internal transitional plumbing only if it does not let Profile selection influence Pod identity and is reported clearly.

Non-goals / constraints:
- Do not migrate/rename existing Pod metadata automatically.
- Do not implement archive/fresh-start UX; that belongs to `add-pod-archive-fresh-start-path`.
- Do not make `.yoi` alone a memory-root marker again.
- Do not edit main-workspace Ticket/workflow/docs records from the coder Pod.
- Do not add compatibility layers solely to preserve Profile-derived identity.

Escalate if:
- Removing `--profile-pod-name` requires a public compatibility policy.
- Existing Pod metadata needs automatic migration or destructive modification.
- Any caller genuinely cannot supply runtime workspace root or Pod identity without reintroducing Profile-derived naming.
- The implementation would weaken explicit `--pod` precedence or make Profile selection influence Pod identity.
- The work expands into broad Pod archive/fresh-start behavior or memory-root redesign.

Validation:
- Focused tests around profile resolution and Pod naming, including non-`yoi` workspace cases.
- Relevant client/tui/pod tests for spawn/profile/Ticket role/Panel lifecycle paths.
- `cargo test -p manifest profile --lib` or corrected equivalent manifest profile test invocation.
- `cargo fmt --check`.
- `git diff --check`.
- `target/debug/yoi ticket doctor`.
- Because startup/profile/runtime resources are touched, final validation should include `nix build .#yoi` before completion unless explicitly waived.

Current code map:
- `crates/manifest/src/profile.rs`: `DEFAULT_POD_NAME`, `derive_pod_name`, profile resolution options/tests.
- `crates/client/src/spawn.rs`: `SpawnConfig`, runtime command argument construction, profile/name behavior.
- `crates/pod/src/entrypoint.rs`: `--profile-pod-name`, profile loading path, CLI tests.
- `crates/tui/src/spawn.rs` and `crates/tui/src/multi_pod.rs`: TUI spawn form, workspace companion/orchestrator lifecycle, `resume_by_pod_name` call sites.
- `crates/client/src/ticket_role.rs`: Ticket role launch naming/profile boundary.
- `crates/tui/src/workspace_panel.rs`: workspace companion/orchestrator Pod name helpers.
- Memory resolution code must be treated as invariant-preserving unless a focused test requires wiring runtime workspace root input.

Critical risks / reviewer focus:
- Profile selector must not influence Pod identity through any fallback path.
- Explicit `--pod` must remain authoritative.
- Non-`yoi` workspace tests must fail on the old behavior.
- Panel Companion must not recreate the `companion`-named bug.
- Ticket role and Orchestrator names must remain explicit and distinct from Companion defaults.
- Memory root and project root must not be collapsed.

---

<!-- event: state_changed author: orchestrator at: 2026-06-08T00:57:20Z from: queued to: inprogress reason: orchestrator_acceptance field: workflow_state -->

## State changed

Accepted queued implementation bundle after shared preflight boundary and routing IntentPacket were recorded. This acceptance precedes worktree creation and coder/reviewer Pod spawning.

---

<!-- event: plan author: orchestrator at: 2026-06-08T00:58:10Z -->

## Plan

Implementation routing accepted and worktree created.

Worktree plan:
- Branch: `runtime-workspace-context`
- Worktree: `.worktree/runtime-workspace-context`
- Base: `9df7f4e ticket: record workspace identity routing`
- Scope: coordinated implementation bundle for `introduce-runtime-workspace-root-context`, `remove-profile-derived-pod-names`, and `use-workspace-basename-for-default-pod-name`.

Delegation plan:
- Spawn sibling coder Pod with write scope limited to `.worktree/runtime-workspace-context`.
- Coder must run Bash commands from the child worktree, not from the main workspace.
- Coder must not edit main-workspace `.yoi` / Ticket / workflow / docs records, and must not create generated memory/local/runtime/secret-like files.
- Reviewer will be spawned read-only after coder reports diff/commits and validation evidence.

Stop condition:
- This Orchestrator will stop at a merge-ready dossier; no merge, close, final approval, or cleanup in this phase.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-08T00:59:49Z -->

## Implementation report

Implementation delegation blocked after inprogress acceptance.

What succeeded:
- Transitioned the coordinated bundle Tickets from `queued` to `inprogress` before implementation side effects.
- Created branch/worktree `runtime-workspace-context` at `.worktree/runtime-workspace-context`.

Blocker:
- Spawning the sibling coder Pod with narrow delegated scope to the child worktree failed:
  - requested scope: `/home/hare/Projects/yoi/.worktree/runtime-workspace-context`
  - error: `requested scope ... is not within spawner yoi-orchestrator's effective scope`
- Retrying at `/home/hare/Projects/yoi/.worktree` failed with the same scope-boundary error.

Current state:
- No coder/reviewer Pods were spawned.
- No implementation edits have been made in the worktree.
- The Ticket remains `inprogress` because the Orchestrator already accepted the queued bundle and created the worktree; this is now an in-progress delegation/scope recovery issue rather than a queued routing issue.

Next recovery options:
- Relaunch/restore an Orchestrator whose delegable scope includes `.worktree/**` and spawn the coder with narrow worktree scope.
- Or explicitly authorize a different delegation scope/path if the current scope policy intentionally excludes `.worktree`.

No merge, close, cleanup, or final approval has occurred.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-08T01:04:44Z -->

## Implementation report

Coder delegation retried after scope authorization.

Result:
- Spawned sibling coder Pod: `coder-runtime-workspace-context-2`.
- Delegated write scope: `/home/hare/Projects/yoi/.worktree/runtime-workspace-context`.
- Added a non-recursive read grant on `/home/hare/Projects/yoi` only because the spawned runtime currently validates its launch cwd; coder was instructed not to inspect/edit main-workspace files and to run all commands from the child worktree.

Coder packet:
- Implements the coordinated runtime workspace / Pod identity bundle for this Ticket plus `remove-profile-derived-pod-names` and `use-workspace-basename-for-default-pod-name`.
- Must preserve the shared boundary decision: `--workspace` runtime workspace root, `--pod` runtime Pod identity, `--profile` recipe selection only, no Profile-derived Pod names.
- Must stop after implementation report; no merge, close, final approval, or main-workspace Ticket edits.

Reviewer delegation remains pending until coder reports diff/commits and validation evidence.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-08T01:36:01Z -->

## Implementation report

Coder completed and reviewer delegated.

Coder result:
- Pod: `coder-runtime-workspace-context-2`
- Commit: `b6af761 runtime: separate workspace pod and profile identity`
- Worktree status: clean after commit.
- Diff against `develop`: 9 files changed, 479 insertions, 216 deletions.
- Coder reported successful focused tests, `cargo check -q`, `cargo fmt --check`, `git diff --check`, `cargo run -q -p yoi -- ticket doctor`, and `nix build .#yoi`.

Files touched by coder:
- `crates/manifest/src/profile.rs`
- `crates/client/src/spawn.rs`
- `crates/client/src/ticket_role.rs`
- `crates/pod/src/entrypoint.rs`
- `crates/tui/src/lib.rs`
- `crates/tui/src/multi_pod.rs`
- `crates/tui/src/single_pod.rs`
- `crates/tui/src/spawn.rs`
- `crates/yoi/src/main.rs`

Reviewer delegation:
- Spawned sibling reviewer Pod: `reviewer-runtime-workspace-context`.
- Reviewer scope: read-only child worktree plus non-recursive parent-root read required by launch cwd validation.
- Reviewer was instructed to judge against the recorded shared boundary decisions and acceptance criteria, not unrecorded preferred tactics.

Pending:
- Await reviewer verdict before merge-ready dossier.
- No merge, close, final approval, or cleanup has occurred.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-08T01:43:05Z -->

## Implementation report

Reviewer returned `request_changes`; coder fix-loop delegated.

Reviewer verdict:
- `request_changes`

Blockers:
1. Client spawn emits `--session ... --pod ...`, but `crates/pod/src/entrypoint.rs` still rejects `--session` with `--pod`.
2. Top-level `yoi --session ... --pod ...` is still rejected in `crates/yoi/src/main.rs`, contradicting the binding decision that `--pod` is runtime identity for restore and fresh create.
3. Hidden `--session-pod-name` remains in `crates/pod/src/entrypoint.rs` as a session-specific identity path, preserving the old split this bundle is meant to remove/collapse.

Reviewer evidence:
- Reviewed diff `develop...b6af761`.
- Ran focused tests for client/pod/yoi/tui/manifest/memory paths.
- Focused CLI probes showed both top-level and `yoi pod` restore forms reject `--session ... --pod ...`.

Non-blocking reviewer observations:
- Profile-derived identity removal looked aligned.
- Panel Companion/Orchestrator naming looked aligned.
- Ticket role explicit names looked aligned.
- Memory-root behavior did not appear conflated.

Action taken:
- Sent fix-loop instructions to `coder-runtime-workspace-context-2` with the three blockers and required focused tests.
- Reviewer approval is pending after coder fix report and re-review.

No merge, close, final approval, or cleanup has occurred.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-08T01:49:15Z -->

## Implementation report

Coder fix-loop completed and reviewer re-review delegated.

Coder follow-up:
- Commit: `15f54df runtime: use pod flag for session identity`
- Worktree: clean on branch `runtime-workspace-context`.
- Fixes claimed:
  - Removed hidden `--session-pod-name` identity path.
  - Allowed Pod runtime `--session <uuid> --pod <name>` and made `--pod` the session identity flag.
  - Allowed top-level `yoi --session <uuid> --pod <name>` and passed explicit Pod name through `LaunchMode::ResumeWithSession`.
  - Added focused tests for top-level parser acceptance, Pod CLI acceptance, and hidden alias rejection.

Coder validation reported:
- focused manifest/client/pod/yoi/tui tests
- `cargo check -q`
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `nix build .#yoi`

Action taken:
- Sent updated commits and prior blocker checklist to `reviewer-runtime-workspace-context` for read-only re-review.

Pending:
- Await reviewer verdict before merge-ready dossier.
- No merge, close, final approval, or cleanup has occurred.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-08T01:53:43Z -->

## Implementation report

Merge-ready dossier: runtime workspace / Pod identity bundle

Ticket id/slug:
- Primary: `20260608-000047-introduce-runtime-workspace-root-context` / `introduce-runtime-workspace-root-context`
- Related: `20260607-235442-remove-profile-derived-pod-names` / `remove-profile-derived-pod-names`
- Related: `20260607-235505-use-workspace-basename-for-default-pod-name` / `use-workspace-basename-for-default-pod-name`

Branch/worktree:
- Branch: `runtime-workspace-context`
- Worktree: `.worktree/runtime-workspace-context`
- Base routing commit: `9df7f4e ticket: record workspace identity routing`
- Current branch commits:
  - `b6af761 runtime: separate workspace pod and profile identity`
  - `15f54df runtime: use pod flag for session identity`

Intent / invariant check:
- `--workspace <path>` is runtime workspace root.
- `--pod <name>` is runtime Pod identity for restore and fresh create.
- `--profile <selector>` remains reusable Profile recipe selection only.
- Profile fields, Profile slug/source, and registry names no longer define or imply `pod.name` in the reviewed startup/profile paths.
- Default Pod name derives from sanitized runtime workspace basename with generic fallback.
- Workspace Orchestrator remains distinct from Companion/default workspace Pod.
- Ticket role/task Pods keep explicit launcher-provided names.
- `.yoi` project records, `.yoi/memory` memory root, and runtime workspace root were not collapsed.
- Existing Pod metadata migration/archive/fresh-start UX remains out of scope.

Implementation summary:
- Added explicit workspace/default Pod naming helpers in profile/startup paths.
- Removed Profile-derived default Pod naming fallback.
- Reworked client spawn, TUI spawn/multi-pod/single-pod, Pod entrypoint, Ticket role launcher, and top-level CLI parsing so runtime workspace root, Pod identity, and Profile selector are separate.
- Preserved explicit role/task Pod names.
- Fixed reviewer blocker by making `--session ... --pod ...` accepted at top-level and Pod runtime boundaries and removing hidden `--session-pod-name` split.
- Added focused regression tests covering non-`yoi` workspace/default naming, `project:companion`, explicit `--pod` precedence, and session restore identity parsing.

Coder / reviewer Pods:
- Coder: `coder-runtime-workspace-context-2`
- Reviewer: `reviewer-runtime-workspace-context`

Review evidence:
- Initial reviewer verdict: `request_changes` for stale session restore identity split.
- Coder fix commit: `15f54df runtime: use pod flag for session identity`.
- Re-review verdict: `approve`.
- Reviewer verified `--session ... --pod ...` parser/runtime acceptance, removal/rejection of `--session-pod-name`, explicit `--pod` restore identity propagation, Panel Companion/Orchestrator naming consistency, Ticket role explicit naming, and no memory/project/workspace root conflation in reviewed paths.

Validation performed by coder and/or reviewer:
- `cargo test -q -p manifest profile`
- `cargo test -q -p client spawn`
- `cargo test -q -p pod entrypoint`
- `cargo test -q -p yoi parse_`
- `cargo test -q -p yoi parse_session_accepts_explicit_runtime_pod_identity`
- `cargo test -q -p tui workspace_panel`
- `cargo test -q -p tui spawn`
- `cargo check -q`
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `nix build .#yoi`
- Orchestrator also checked child worktree status, branch commits, diff stat, and `git diff --check develop...HEAD`.

Blockers fixed or rejected findings:
- Fixed: Pod runtime rejected `--session ... --pod ...`.
- Fixed: top-level `yoi --session ... --pod ...` was rejected.
- Fixed: hidden `--session-pod-name` preserved an old identity split.
- No rejected reviewer blockers remain.

Residual risks:
- Broader live restore behavior still lacks a full existing-session E2E fixture; reviewer judged parser/runtime wiring and focused unit coverage sufficient for this Ticket.
- Final merge validation should still be run from main after merge because this touches CLI/startup/profile/runtime paths and Nix packaging.

Dirty state:
- Main workspace was clean before recording this dossier.
- Child worktree `.worktree/runtime-workspace-context` is clean at `15f54df`.
- Coder/reviewer Pods are currently idle and still available for follow-up unless the merge-completion owner chooses to stop/reclaim them.

Parent/human decision needs:
- Merge authority is still required. This Orchestrator routing/review phase stops here without merge, close, final main-branch approval, or cleanup.

---

<!-- event: review author: orchestrator at: 2026-06-08T01:59:52Z status: approve -->

## Review: approve

Final merge-completion approval after merge to `develop` and post-merge validation.

Evidence:
- Merged branch `runtime-workspace-context` with `--no-ff`.
- Reviewer `reviewer-runtime-workspace-context` approved after coder fix-loop.
- Post-merge validation passed: focused manifest/client/pod/yoi/tui tests, `cargo check -q`, `cargo fmt --check`, `git diff --check`, `cargo run -q -p yoi -- ticket doctor`, and `nix build .#yoi`.
- Coder/reviewer Pods stopped and delegated scope reclaimed.
- Merged worktree removed and branch deleted.

This approval is for the merged main-branch result, not merely the branch-local reviewer verdict.

---

<!-- event: state_changed author: orchestrator at: 2026-06-08T01:59:59Z from: inprogress to: done reason: merged_and_validated field: workflow_state -->

## State changed

Merged to `develop`, post-merge validation passed, final merge-completion approval recorded, and runtime-workspace branch/worktree/Pods cleaned up.

---

<!-- event: close author: hare at: 2026-06-08T02:00:18Z status: closed -->

## Closed

Merged and completed the coordinated runtime workspace / Pod identity bundle.

Summary:
- Introduced explicit separation of runtime workspace root, runtime Pod identity, and Profile selector across startup/profile/client/TUI/Pod paths.
- Removed Profile-derived Pod identity fallback and preserved Profile as recipe selection only.
- Default Pod identity now derives from runtime workspace basename with generic fallback; explicit `--pod` remains authoritative.
- `--session ... --pod ...` is accepted for restore, and hidden `--session-pod-name` was removed.
- Panel Companion/Orchestrator and Ticket role naming remain explicit and distinct.

Merged branch/worktree:
- Branch: `runtime-workspace-context`
- Commits: `b6af761`, `15f54df`
- Merge commit on `develop`: `b7a533f`

Validation passed after merge:
- focused manifest/client/pod/yoi/tui tests
- `cargo check -q`
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `nix build .#yoi`

Cleanup completed:
- Stopped coder/reviewer Pods and reclaimed scope.
- Removed `.worktree/runtime-workspace-context`.
- Deleted branch `runtime-workspace-context`.

---
