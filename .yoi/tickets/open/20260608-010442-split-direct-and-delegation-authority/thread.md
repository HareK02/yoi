<!-- event: create author: LocalTicketBackend at: 2026-06-08T01:04:42Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: decision author: intake at: 2026-06-08T03:31:19Z -->

## Decision

## Intake classification

- readiness: requirements_sync_needed for implementation, but ready for Orchestrator routing to preflight/design synchronization.
- needs_preflight: true
- risk_flags: [authority-boundary, scope-permission, profile-manifest-resolution, pod-metadata-persistence, spawnpod-policy, backward-compatibility, diagnostics]

This Ticket touches authority boundaries and has explicit unresolved design questions, so it should not be routed directly to implementation. The next routing should be preflight/design before any coder worktree or implementation Pod is started.

---

<!-- event: intake_summary author: intake at: 2026-06-08T03:31:25Z -->

## Intake summary

Existing Ticket refined rather than duplicated. Scope is to split direct Pod/tool authority from delegation authority for child Pod spawn scopes, preserving explicit parent grants and deny behavior. Intake classification: ready for Orchestrator routing, not direct implementation; route to preflight/design first because the Ticket touches authority boundaries, profile/manifest scope resolution, Pod metadata/restoration behavior, SpawnPod policy, backward compatibility, and diagnostics. needs_preflight: true. risk_flags: [authority-boundary, scope-permission, profile-manifest-resolution, pod-metadata-persistence, spawnpod-policy, backward-compatibility, diagnostics].

---

<!-- event: state_changed author: intake at: 2026-06-08T03:31:25Z from: intake to: ready reason: intake_ready field: workflow_state -->

## State changed

Intake complete for routing. The Ticket is ready to queue for Orchestrator preflight/design routing; implementation should not start until the unresolved authority-boundary questions are settled or explicitly bounded.

---

<!-- event: decision author: intake at: 2026-06-08T05:44:05Z -->

## Decision

## Implementation direction: carrying delegation authority in Pod state

Adopt a split where direct scope and delegation scope are separate authority surfaces.

- Keep existing `scope` as direct tool/filesystem authority for the Pod itself.
- Add a separate `delegation_scope` / delegation grant surface for authority the Pod may pass to children through `SpawnPod`.
- Do not derive delegation authority implicitly from direct write scope. If `delegation_scope` is absent, the Pod cannot delegate write scope.
- Do not persist a mutable "remaining delegation scope" in Pod metadata. Store static grants in the resolved manifest snapshot, and use `spawned_children` as the durable record of currently consumed delegated scope.
- On restore, derive:
  - direct effective scope = `manifest.scope` plus write-deny rules for outstanding `spawned_children` delegations, preserving the current direct-write conflict behavior;
  - delegation effective scope = `manifest.delegation_scope` minus outstanding `spawned_children` delegations.
- Runtime `Pod` state should carry separate shared handles for direct scope and delegation scope. Built-in filesystem tools continue to consult direct scope; `SpawnPodTool` validates against delegation scope.
- `pod-registry::Allocation` should gain a delegation grant field so `delegate_scope()` checks subset against delegation grants rather than direct `scope_allow`. Registry conflict ownership should still prevent overlapping live write allocations.
- Existing `spawned_children` remains the durable dynamic authority-consumption record; `spawned_pods.json` remains only a runtime mirror/hint.

This supports the target role pattern: Companion/Intake/Reviewer default to direct read with no write delegation; Orchestrator can have limited/no direct workspace write while still having explicit bounded delegation grants for child worktrees; Coder receives direct child-worktree write and no further delegation unless explicitly granted.

---

<!-- event: state_changed author: workspace-panel at: 2026-06-08T05:45:43Z from: ready to: queued reason: queued field: workflow_state -->

## State changed

Ticket queued for Orchestrator routing.


---

<!-- event: decision author: orchestrator at: 2026-06-08T05:47:04Z -->

## Decision

Routing decision: implementation_ready

Reason:
- The Ticket thread contains an explicit implementation direction that resolves the authority-boundary question: every Pod has direct operation authority, while only a separately granted delegation capability authorizes `SpawnPod` scope subdivision.
- The binding decisions distinguish direct read/write scope from delegation grants, define default no-delegation behavior, preserve direct Bash/Edit/Write access, and require clear SpawnPod diagnostics.
- Remaining uncertainty is bounded implementation design: exact type names, Profile/manifest field names, metadata persistence shape, and test organization.

Evidence checked:
- Ticket body: problem, goals, requirements, non-goals/constraints, acceptance criteria, and validation expectations.
- Thread: Intake summary, implementation direction, binding decisions/invariants, and latest `ready -> queued` event.
- Workspace state: no matching branch/worktree exists; main workspace has unrelated Ticket-record edits and newly queued panel Ticket records outside this implementation path.
- Code map search for `SpawnPod`, scope/delegation validation, profile/manifest scope fields, pod metadata/scope persistence, and child scope allocation.
- Ticket doctor: 0 errors; existing warnings are unrelated legacy closed-Ticket diagnostics.

IntentPacket:

Intent:
- Split Pod direct operation authority from subdelegation authority so a Pod may directly read/write broad scope without automatically being allowed to spawn children over that scope.

Binding decisions / invariants:
- Direct scope authorizes the current Pod's own tools only.
- Delegation scope/capability authorizes only what that Pod may pass to `SpawnPod` children.
- Default delegation authority is none unless explicitly granted.
- Broad direct workspace/write scope must not imply broad child delegation.
- `SpawnPod` must validate requested child scope against the parent's delegation grant, not the parent's direct tool scope.
- Existing direct operations (`Read`, `Write`, `Edit`, `Bash`) must keep using direct scope and must not be reduced by this split.
- Existing/restored older Pods without delegation metadata must be unable to spawn children with delegated filesystem scope until granted by explicit new configuration/metadata.
- Profile/workspace role defaults should grant delegation intentionally only to roles that are supposed to orchestrate children.
- Do not weaken scope path validation, registry accounting, child scope reclaim, or host-authority checks.

Requirements / acceptance criteria:
- Introduce a typed representation for direct scope versus delegation/subdelegation scope.
- Update Profile/manifest/runtime config parsing so role Profiles can express broad direct scope with narrow/no delegation authority.
- Update Pod metadata/session persistence and restore so effective direct scope and delegation grant are durable/replayable.
- Update `SpawnPod` validation and diagnostics to use delegation authority and say when delegation is missing/insufficient.
- Update local role/Profile configuration for Companion/Intake/Orchestrator/Coder/Reviewer according to intended delegation behavior.
- Preserve existing scope validation for direct tools.
- Add tests covering: direct tool access allowed while SpawnPod delegation denied; explicit delegation grant permits appropriate subset; over-delegation rejected; old metadata/restored scope without delegation defaults to no delegation.

Implementation latitude:
- Coder may choose exact Rust type names and config field names if they clearly express direct vs delegation authority.
- Coder may stage internal compatibility for parsing older metadata as direct-scope-only/no-delegation, but must not create broad old-name delegation aliases.
- Coder may keep existing user-facing scope terminology where it remains clear, but diagnostics must distinguish direct authority from delegation authority.

Escalate if:
- The split requires a broad migration of stored metadata beyond defaulting missing delegation to none.
- Existing orchestrator/role Profiles cannot express needed delegation without a separate config model.
- Implementing this would require changing host authority grants, Ticket backend authority, or non-filesystem capabilities.
- Tests reveal SpawnPod registry/reclaim semantics assume direct scope and delegation scope are the same in a way that cannot be localized.

Validation:
- Focused tests for scope/delegation validation and SpawnPod over-delegation diagnostics.
- Profile/manifest parsing tests for direct/delegation fields.
- Pod metadata/session restore tests for missing delegation defaulting to none.
- `cargo test -p pod ...` / `cargo test -p manifest ...` / `cargo test -p client ...` focused equivalents selected by coder.
- `cargo check -q`.
- `cargo fmt --check`.
- `git diff --check`.
- `cargo run -q -p yoi -- ticket doctor`.
- Because scope/config/runtime/profile behavior is touched, final merge-completion must include `nix build .#yoi`.

Current code map:
- `crates/pod/src/tools/pod_management.rs`: `SpawnPod` input and requested child scope handling.
- `crates/pod/src/session_store.rs`, `crates/pod/src/pod_registry.rs`, and related metadata/session scope snapshot code.
- `crates/manifest/src/*`: scope allow parsing/profile/manifest config.
- `crates/tui/src/spawn.rs`, `crates/client/src/spawn.rs`, `crates/client/src/ticket_role.rs`: runtime launch/profile/scope propagation.
- Project role Profiles under `.yoi/profiles` / profile resources as applicable.

Critical risks / reviewer focus:
- Direct write scope must remain usable by the parent Pod after the split.
- `SpawnPod` must not fall back to direct scope when delegation grant is absent.
- Missing old delegation metadata must fail closed for child delegation.
- Orchestrator/coder/reviewer role configs must not accidentally grant broad delegation to non-orchestrator roles.
- Error messages must make the missing delegation grant understandable, not look like ordinary read/write scope denial.

---

<!-- event: state_changed author: orchestrator at: 2026-06-08T05:47:15Z from: queued to: inprogress reason: orchestrator_acceptance field: workflow_state -->

## State changed

Accepted queued implementation after reading the Ticket, implementation-direction decision, workspace state, and authority/scope code map. This acceptance precedes worktree creation and coder/reviewer Pod spawning.

---

<!-- event: plan author: orchestrator at: 2026-06-08T05:47:43Z -->

## Plan

Implementation routing accepted and worktree created.

Worktree plan:
- Branch: `split-direct-and-delegation-authority`
- Worktree: `.worktree/split-direct-and-delegation-authority`
- Base: `fa39f92 ticket: route direct delegation authority split`
- Scope: split direct tool scope from child-delegation scope for `SpawnPod`, profile/runtime metadata, and diagnostics.

Delegation plan:
- Spawn sibling coder Pod with write scope limited to `.worktree/split-direct-and-delegation-authority`.
- Coder must run Bash commands from the child worktree, not from the main workspace.
- Coder must not edit main-workspace `.yoi` / Ticket / workflow / docs records, and must not create generated memory/local/runtime/secret-like files.
- Reviewer will be spawned read-only after coder reports diff/commits and validation evidence.

Concurrency note:
- Newly queued panel/Ticket-language/shutdown Tickets exist in main workspace records, but this implementation must remain isolated to the child worktree and should not edit unrelated Ticket records.

Stop/completion policy:
- Because the human authorized cleanup after completion, this Orchestrator will proceed through reviewer approval, merge-completion validation, Ticket close, Pod stop, worktree removal, and branch deletion if the branch is approved and merge authority remains clear.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-08T06:24:20Z -->

## Implementation report

Coder completed and reviewer delegated.

Coder result:
- Pod: `coder-split-direct-delegation`
- Commit: `a4a9b00 feat: split direct and delegation scope authority`
- Worktree status: clean after commit.
- Diff against `develop`: 9 files changed, 320 insertions, 20 deletions.

Files touched by coder:
- `.yoi/profiles/_base.lua`
- `.yoi/profiles/orchestrator.lua`
- `crates/manifest/src/config.rs`
- `crates/manifest/src/lib.rs`
- `crates/manifest/src/profile.rs`
- `crates/manifest/src/scope.rs`
- `crates/pod/src/pod.rs`
- `crates/pod/src/spawn/tool.rs`
- `crates/pod/tests/spawn_pod_test.rs`

Coder reported implementation:
- Added `delegation_scope` to manifest/profile resolution separately from direct `scope`.
- Added `DelegationScope` type and changed `SpawnPod` requested child scope validation to check delegation grant rather than direct tool scope.
- Missing `delegation_scope` in old metadata/snapshots defaults to no delegation.
- `inherit` Profile does not implicitly inherit subdelegation authority.
- `.yoi/profiles/orchestrator.lua` explicitly gets workspace-write delegation; Companion/Intake/Coder/Reviewer remain without delegation grant.
- Added tests for direct scope with delegation denied, explicit grant success, over-delegation rejection diagnostics, old snapshot no-delegation default, and Profile Lua delegation resolution.

Coder reported validation:
- `cargo test -p manifest profile --lib`
- `cargo test -p manifest deserialize_old_manifest_snapshot_defaults_to_no_delegation --lib`
- `cargo test -p manifest --lib`
- `cargo test -p pod spawn_pod --test spawn_pod_test`
- `cargo test -p pod-registry`
- `cargo check -q`
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `nix build .#yoi`

Reviewer delegation:
- Spawned sibling reviewer Pod: `reviewer-split-direct-delegation`.
- Reviewer scope: read-only child worktree plus non-recursive parent-root read required by launch cwd validation.
- Reviewer was instructed to judge against the recorded Ticket requirements and binding decisions.

Pending:
- Await reviewer verdict before merge-ready dossier / merge-completion.
- No merge, close, final approval, or cleanup has occurred for this Ticket.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-08T06:31:13Z -->

## Implementation report

Reviewer returned `request_changes`; coder fix-loop delegated.

Reviewer verdict:
- `request_changes`

Blocker:
- `DelegationScope::allows_rule` / `rule_covers` over-approves subset delegation for `recursive = false` parent grants.
- A parent grant such as `/repo` with `recursive = false` permits only `/repo` and direct children, but a requested child grant `/repo/child` with `recursive = false` permits `/repo/child` plus its direct children. That includes `/repo/child/grandchild`, which exceeds the parent grant.
- Current `direct_child(&requested.target, &available.target)` coverage treats this as allowed, weakening path validation.

Related reviewer concern:
- Deny overlap logic around the same code may under-detect overlaps for non-recursive deny versus recursive request, e.g. deny `/repo` non-recursive should overlap a recursive request rooted at `/repo/child` because the deny covers `/repo/child`.

Action taken:
- Sent fix-loop instructions to `coder-split-direct-delegation` with required subset/overlap tests for recursive/non-recursive semantics.

Pending:
- Await coder fix report and reviewer re-review.
- No merge, close, final approval, or cleanup has occurred.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-08T06:38:27Z -->

## Implementation report

Coder fix-loop completed and reviewer re-review delegated.

Coder follow-up:
- Commit: `f43c8ac fix: validate delegation path sets exactly`
- Worktree: clean on branch `split-direct-and-delegation-authority`.
- Fixes claimed:
  - `DelegationScope` subset checks now compare recursive/non-recursive path-set inclusion rather than only rule target containment.
  - `recursive=false /repo` allows exact `recursive=false /repo` but rejects `recursive=false /repo/child`.
  - `recursive=true /repo` allows `recursive=false /repo/child`.
  - deny overlap now compares path sets, so deny `recursive=false /repo` rejects request `recursive=true /repo/child`.
  - Added focused regression tests in `crates/manifest/src/scope.rs` and `crates/pod/tests/spawn_pod_test.rs`.

Coder validation reported:
- `cargo test -p manifest delegation_ --lib`
- `cargo test -p manifest --lib`
- `cargo test -p pod spawn_pod --test spawn_pod_test`
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `nix build .#yoi`

Action taken:
- Sent updated commits and prior blocker checklist to `reviewer-split-direct-delegation` for read-only re-review.

Pending:
- Await reviewer verdict before merge-ready dossier / merge-completion.
- No merge, close, final approval, or cleanup has occurred.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-08T06:40:40Z -->

## Implementation report

Merge-ready dossier: direct/delegation authority split

Ticket id/slug:
- `20260608-010442-split-direct-and-delegation-authority` / `split-direct-and-delegation-authority`

Branch/worktree:
- Branch: `split-direct-and-delegation-authority`
- Worktree: `.worktree/split-direct-and-delegation-authority`
- Current branch commits:
  - `a4a9b00 feat: split direct and delegation scope authority`
  - `f43c8ac fix: validate delegation path sets exactly`

Intent / invariant check:
- Direct scope and delegation/subdelegation scope are now represented separately.
- Direct scope continues to authorize the current Pod's own tools.
- `SpawnPod` validates requested child scope against delegation authority, not direct scope.
- Missing old delegation metadata defaults to no delegation/fail-closed for child delegation.
- Broad direct workspace/write scope no longer implies broad child delegation.
- Role defaults intentionally grant delegation to Orchestrator only in the project profile updates; Companion/Intake/Coder/Reviewer remain without broad delegation.
- Registry allocation/reclaim and direct tool scope semantics were not intentionally weakened.

Implementation summary:
- Added `delegation_scope` to manifest/profile resolution separately from direct `scope`.
- Added `DelegationScope` representation and validation helpers in manifest scope handling.
- Updated Pod runtime/config snapshot paths to carry delegation grants durably/replayably with missing delegation defaulting to none.
- Updated `SpawnPod` validation/diagnostics to use delegation grant.
- Updated project role Profiles so Orchestrator explicitly receives workspace-write delegation and base/non-orchestrator roles do not inherit it.
- Added regression tests for no-delegation denial, explicit delegation success, over-delegation rejection, missing-old-metadata default, Profile Lua delegation resolution, recursive/non-recursive path-set subset handling, and deny overlap behavior.

Files touched:
- `.yoi/profiles/_base.lua`
- `.yoi/profiles/orchestrator.lua`
- `crates/manifest/src/config.rs`
- `crates/manifest/src/lib.rs`
- `crates/manifest/src/profile.rs`
- `crates/manifest/src/scope.rs`
- `crates/pod/src/pod.rs`
- `crates/pod/src/spawn/tool.rs`
- `crates/pod/tests/spawn_pod_test.rs`

Coder / reviewer Pods:
- Coder: `coder-split-direct-delegation`
- Reviewer: `reviewer-split-direct-delegation`

Review evidence:
- Initial reviewer verdict: `request_changes` for recursive=false path-set over-delegation and deny-overlap under-detection.
- Coder fix commit: `f43c8ac fix: validate delegation path sets exactly`.
- Re-review verdict: `approve`.
- Reviewer confirmed `recursive=false /repo` no longer permits `recursive=false /repo/child`, `recursive=true /repo` permits `recursive=false /repo/child`, deny `recursive=false /repo` overlaps request `recursive=true /repo/child`, direct/delegation split remains intact, missing delegation metadata fails closed, and role grants remain intentional.

Validation performed by coder and/or reviewer:
- `cargo test -p manifest profile --lib`
- `cargo test -p manifest deserialize_old_manifest_snapshot_defaults_to_no_delegation --lib`
- `cargo test -p manifest delegation_ --lib`
- `cargo test -p manifest --lib`
- `cargo test -p pod spawn_pod --test spawn_pod_test`
- `cargo test -p pod-registry`
- `cargo check -q`
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `nix build .#yoi`

Blockers fixed or rejected findings:
- Fixed: recursive=false delegation subset over-approval.
- Fixed: non-recursive deny vs recursive request overlap under-detection.
- No remaining reviewer blockers.

Residual risks:
- The implementation conservatively rejects delegating non-recursive direct-child scope from a non-recursive parent grant because path validation is path-based rather than file/directory-aware. Reviewer accepted this safety-first behavior.
- Future support for child Pods that themselves may subdelegate will require an explicit separate child-delegation request/validation/persistence surface; this is outside the current Ticket.

Dirty state:
- Child worktree is clean at `f43c8ac`.
- Main workspace has unrelated Ticket-record edits for queued/preflight work; they are outside this branch's touched paths and are understood.

Parent/human decision needs:
- User has authorized merge-completion and cleanup after approved work. Proceeding to merge-completion unless post-merge validation fails.

---
