<!-- event: create author: "yoi ticket" at: 2026-07-10T16:11:33Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: decision author: hare at: 2026-07-10T16:12:30Z -->

## Decision

Follow-up to 00001KX6BPY7M. Deletion/cleanup policy is intentionally manual-first:

- No automatic prune based on age or capacity in this ticket.
- Backend presents a plan with reasons, blockers, linked Worker/Workdir ids, retention, pinned state, and expected revision/digest.
- User or explicit orchestration authority selects targets and executes cleanup.
- Pinned Worker history is protected.
- Clean workdir files are cache and may be manually cleaned when not linked to running Workers.
- Dirty orphan workdirs require recovery/discard decision, not normal prune.


---

<!-- event: decision author: hare at: 2026-07-10T16:31:05Z -->

## Decision

Clarification: because 00001KX6BPY7M is already in progress, this follow-up ticket explicitly owns implementation of Worker pinned state beyond merely consuming it in prune logic.

Scope added here:
- Persist pinned flag or equivalent Worker retention field in Backend registry.
- Add Backend Worker pin/unpin mutation API.
- Include pinned state in Worker list/detail and prune plan.
- Ensure manual cleanup execution rejects pinned Worker history and protected linked records.


---

<!-- event: decision author: hare at: 2026-07-10T16:43:34Z -->

## Decision

Lifecycle clarification:

- Worker lifecycle should stay simple: running -> stopped -> delete.
- Do not introduce archived as a Worker lifecycle state in this ticket.
- Worker is the storage unit for Session/transcript history; deleting a Worker deletes or removes access to that history.
- Pinned remains necessary and blocks Worker deletion.
- Workdir files are cache and should be manually deletable.
- Dirty workdir deletion is allowed only with explicit confirmation that changes will be discarded.
- Compression/archive storage for Session history is out of scope and should be a separate storage policy if needed later.


---

<!-- event: intake_summary author: hare at: 2026-07-10T16:44:17Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-10T16:44:17Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-07-10T16:45:14Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-07-10T16:46:04Z -->

## Decision

Routing decision: blocked_by_dependency_or_missing_authority

Reason:
- Ticket is a manual delete / cleanup follow-up that depends on the Backend Worker/Workdir registry and link authority from `00001KX6BPY7M`。
- `00001KX6BPY7M` is currently `inprogress` on branch `work/00001KX6BPY7M-worker-workdir-registry` and under external review。
- This Ticket touches the same store schema, server APIs, Worker/Workdir projections, retention/managed-unmanaged semantics, and cleanup safety rules。
- Starting this Ticket now would create a high-conflict parallel branch and could implement delete/cleanup behavior against an unreviewed registry model。
- Therefore this routing pass leaves the Ticket queued and does not record `queued -> inprogress`, create a worktree, or spawn role Pods。

Evidence checked:
- Ticket body / thread / artifacts。
- `TicketRelationQuery(00001KX6CRVBE)`: 0 typed relations, but Ticket body/order context makes `00001KX6BPY7M` a practical implementation prerequisite。
- `TicketOrchestrationPlanQuery(00001KX6CRVBE)`: prior record 0 件だったため、今回 `after 00001KX6BPY7M` と waiting-capacity note を記録。
- `TicketList`: queued はこの Ticket 1件、inprogress は `00001KX6BPY7M` 1件。
- Orchestrator worktree git status: clean on `orchestration`。

Next action:
- `00001KX6BPY7M` の implementation/review/merge outcome を待つ。
- registry/link model が merge されたら、この Ticket を再 routing し、updated orchestration branch から start する。

Escalate if:
- 人間が `00001KX6BPY7M` とこの Ticket を同一 combined worktree で統合実装する方針に切り替えたい場合。

---

<!-- event: decision author: orchestrator at: 2026-07-10T17:39:00Z -->

## Decision

Routing decision: accept for implementation

Reason:
- Previously recorded prerequisite `00001KX6BPY7M` has now been reviewed, merged into orchestration branch, final-validated, closed, and its child implementation worktree/branch cleaned up。
- `TicketRelationQuery(00001KX6CRVBE)` shows no typed blockers。
- Queue contains this Ticket and there are no other `inprogress` Tickets。
- Ticket is concrete and well scoped: manual Worker delete / Workdir cleanup plan and execution on top of Backend Worker/Workdir registry/link authority。

Accepted plan:
- Create dedicated implementation worktree `/home/hare/Projects/yoi/.worktree/00001KX6CRVBE-manual-cleanup` on branch `work/00001KX6CRVBE-manual-cleanup` from current orchestration branch。
- Route implementation to sibling Coder Pod。
- Route external review to sibling Reviewer Pod after Coder commit/validation。
- Orchestrator retains merge, final validation, Ticket close, and cleanup authority。

Design constraints for implementation:
- Cleanup/delete is manual-first only; no automatic prune policy。
- Worker lifecycle remains `running -> stopped -> delete`; do not introduce `archived` lifecycle state。
- `pinned` Worker/history is protected in plan generation and execution。
- Workdir file cleanup blocks when linked to running Workers。
- Dirty Workdir discard is explicit and separate from ordinary clean cleanup。
- Execution must reject stale plan revision/digest。
- Browser-facing responses/UI must not expose raw Runtime paths。
- Workdir/Worker canonical records should remain Backend/registry based and safe after execution。

---

<!-- event: state_changed author: orchestrator at: 2026-07-10T17:39:03Z from: queued to: inprogress reason: accepted_for_implementation field: state -->

## State changed

Prerequisite registry/link model is now closed and merged. Accepting queued manual cleanup Ticket for implementation before creating worktree or spawning role Pods.

---

<!-- event: implementation_report author: orchestrator at: 2026-07-10T18:13:40Z -->

## Implementation report

Implementation progress report:

- Coder Pod `yoi-coder-00001KX6CRVBE-manual-cleanup` completed implementation in `/home/hare/Projects/yoi/.worktree/00001KX6CRVBE-manual-cleanup` on branch `work/00001KX6CRVBE-manual-cleanup`。
- Implementation commit: `ba44391acfdb8076c06ced9324b74d6cc63910ab feat: add manual worker workdir cleanup`。
- Orchestrator inspected the worktree status, branch log, commit stats, and `git diff --check 5f762a6c..HEAD`; worktree was clean and diff check passed。

Coder-reported implementation summary:
- Added Backend Worker retention APIs for pin/unpin registry rows; Worker list/detail carries `pinned` / `retention_state`; pin/unpin mutates Backend registry only and does not affect Runtime processes。
- Added manual Runtime cleanup plan API with separate Worker delete and Workdir cleanup/delete candidates, action kind, reason/blocking reason, linked ids, pinned state, cleanliness/file status, running-link state, and safe reclaim-byte placeholder。
- Added manual cleanup execution API requiring expected plan revision/digest; rejects stale plans, pinned Worker/history delete, running-linked Workdir cleanup, and dirty cleanup without explicit discard confirmation; allows safe removed/missing Workdir registry deletion。
- Added Browser UI for manual cleanup preview/execution on Runtime Workdirs page and Backend-only pin/unpin controls on Worker page。
- Added focused tests for plan generation, stale rejection, pinned protection, running-linked Workdir blocking, dirty confirmation, removed/missing record delete, and raw path redaction。

Coder-reported validation passed:
- `git diff --check`
- `cargo test -p yoi-workspace-server --lib`
- `cargo test -p worker-runtime --features ws-server,fs-store`
- `cargo check -p yoi`
- `cd web/workspace && deno task check && deno task test`
- `yoi ticket doctor`
- `nix build .#yoi --no-link`

Next action:
- Route to external Reviewer Pod before merge/close decisions。

---

<!-- event: review author: reviewer at: 2026-07-10T18:19:11Z status: request_changes -->

## Review: request changes

External review result: request_changes

Blocker:
- Dirty Workdir safety is not implemented against real Runtime/filesystem state。
- Cleanup plan decides `WorkdirDirtyDiscard` solely from `WorkdirRegistryRecord.cleanliness` (`crates/workspace-server/src/server.rs:1689-1695`)。
- Runtime observation sync always writes `cleanliness: "clean"` for observed Workdirs (`server.rs:4179-4199`) and there is no code outside tests that records `"dirty"` (`rg "cleanliness.*dirty|dirty.*cleanliness"` only finds plan/test use)。
- Execution then treats such targets as `WorkdirCleanCleanup` and calls Runtime cleanup without dirty confirmation (`server.rs:1851-1852`)。
- Runtime cleanup uses forced removal (`crates/worker-runtime/src/working_directory.rs:505-508`)。
- This violates acceptance items requiring safe dirty/clean status representation and explicit confirmation before dirty discard。

Test gap:
- The dirty confirmation test seeds a synthetic registry row with `cleanliness = "dirty"` (`server.rs:5436-5449`, `5551-5580`), so it does not catch the real path where Runtime/workdir sync overwrites or fails to detect dirty files。

Positive evidence:
- pin/unpin API/routes and retention projection exist (`server.rs:1544-1584`, `2032-2072`, `4008-4022`)。
- plan/execution endpoints include digest/revision stale checks and pinned/running-link blocking (`server.rs:1766-1840`)。
- UI preview/execution and pin controls are present without raw path fields。

Non-blocking note:
- `estimated_reclaim_bytes` is present but always `None`; confirm whether “when safe” requires actual byte estimation before approval。

---

<!-- event: implementation_report author: orchestrator at: 2026-07-10T18:30:52Z -->

## Implementation report

Follow-up implementation progress report after request_changes:

- Coder Pod `yoi-coder-00001KX6CRVBE-manual-cleanup` completed the dirty/unknown Workdir safety fix and committed `361569a69efcba929b59b9c48113add5499d5068 fix: require discard confirmation for unknown workdirs`。
- Orchestrator inspected the implementation worktree status, branch log, commit stats, and `git diff --check 5f762a6c..HEAD`; worktree was clean and diff check passed。

Coder-reported fix summary:
- Runtime-observed/materialized Workdirs are no longer marked `clean` without trusted clean evidence。
- Observed Workdirs now sync as `cleanliness: "unknown"`。
- Cleanup plan downgrades stale `clean` registry rows to `unknown` when real Runtime observation exists。
- Present Workdirs with `cleanliness: "unknown"` or `dirty` are classified as discard-confirmation targets, not normal clean cleanup。
- Direct Workdir cleanup rejects unknown/dirty Workdirs and requires the cleanup execution API with explicit discard confirmation。
- Browser UI labels distinguish verified-clean cleanup from dirty/unknown-state discard。
- `estimated_reclaim_bytes` remains intentionally `None` unless a safe estimate is available。

Regression tests added/updated:
- Real observed Workdir without verified clean evidence is classified as `unknown` and `WorkdirDirtyDiscard`。
- Normal/direct cleanup is rejected for unknown Workdir state。
- Dirty/unknown discard requires explicit confirmation。
- Stale `clean` registry state is downgraded when Runtime observation exists。
- Existing path-safe cleanup test uses explicit cleanup execution confirmation。

Coder-reported validation passed:
- `git diff --check`
- `cargo test -p yoi-workspace-server --lib`
- `cargo test -p worker-runtime --features ws-server,fs-store`
- `cargo check -p yoi`
- `cd web/workspace && deno task check && deno task test`
- `yoi ticket doctor`
- `nix build .#yoi --no-link`

Next action:
- Request focused re-review on the dirty/unknown Workdir blocker and overall acceptance status before merge/close decisions。

---

<!-- event: review author: reviewer at: 2026-07-10T18:36:11Z status: approve -->

## Review: approve

Focused re-review result: approve

Evidence:
- Dirty/unknown blocker is fixed。
- Runtime/materialized Workdirs now sync as `cleanliness: "unknown"` (`crates/workspace-server/src/server.rs:4208`; pending create also around `1400`)。
- Cleanup planning downgrades any live-observed non-dirty row to unknown before action selection (`server.rs:1689-1701`)。
- Unknown/dirty present Workdirs become `WorkdirDirtyDiscard` with explicit-confirmation reasons (`server.rs:1721-1725`)。
- Direct Workdir DELETE rejects that path before calling Runtime cleanup (`server.rs:1491-1509`)。
- Execution still checks expected revision + digest for staleness (`server.rs:1780-1788`), blocks pinned/running-linked Workdirs (`server.rs:1703-1707`, `1834-1839`), and requires `confirm_dirty_discard_target_ids` before forced Runtime cleanup for dirty/unknown targets (`server.rs:1842-1850`)。
- Normal clean cleanup is only reachable for `clean` candidates (`server.rs:1698-1699`, `1860-1861`)。

No observed regressions in reviewed acceptance points:
- Pinned Worker deletion remains blocked (`server.rs:1811-1816`)。
- Workdir record delete remains separate for removed/missing (`server.rs:1696-1697`, `1869-1879`)。
- Plan responses expose ids/status/reasons/links/retention but not raw paths (`server.rs:620-633`)。
- UI labels dirty/unknown discard distinctly from verified-clean cleanup with a separate confirm checkbox (`web/workspace/.../workdirs/+page.svelte`)。
- `estimated_reclaim_bytes: None` is acceptable here because no safe size source is implemented/exposed; the optional field is present and avoids raw-path traversal/leakage。

Validation performed by reviewer:
- `cargo test -p yoi-workspace-server cleanup --lib` passed。
- Targeted regression tests `observed_workdir_without_verified_clean_evidence_requires_discard_confirmation` and `stale_clean_registry_row_is_downgraded_by_real_observation_before_cleanup` passed。
- `git diff --check HEAD^..HEAD` passed。

---

<!-- event: implementation_report author: orchestrator at: 2026-07-10T18:37:23Z -->

## Implementation report

Merge / final validation report:

- Reviewer approved the implementation after the dirty/unknown Workdir safety fix。
- Merged branch `work/00001KX6CRVBE-manual-cleanup` into orchestration branch with merge commit `4970a58c merge: manual worker workdir cleanup`。
- Ticket record conflicts from child branch were resolved in favor of Orchestrator-authoritative Ticket records; implementation code changes were merged。
- Final validation was rerun in `/home/hare/Projects/yoi/.worktree/orchestration` after merge。

Validation passed:
- `git diff --check`
- `cargo test -p yoi-workspace-server --lib`
- `cargo test -p worker-runtime --features ws-server,fs-store`
- `cargo check -p yoi`
- `cd web/workspace && deno task check`
- `cd web/workspace && deno task test`
- `yoi ticket doctor`
- `nix build .#yoi --no-link`

Validation log:
- `/run/user/1000/yoi/yoi-orchestrator/bash-output/manual-cleanup-final-validation-1783708592.txt`

Reviewed/merged implementation commits:
- `ba44391a feat: add manual worker workdir cleanup`
- `361569a6 fix: require discard confirmation for unknown workdirs`

---

<!-- event: state_changed author: orchestrator at: 2026-07-10T18:37:27Z from: inprogress to: done reason: implementation_validated field: state -->

## State changed

Implementation was reviewed, merged into orchestration branch, and final validation passed. Moving to done before closure.

---

<!-- event: state_changed author: hare at: 2026-07-10T18:37:40Z from: done to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-07-10T18:37:40Z status: closed -->

## 完了

Manual Worker/Workdir delete and cleanup operations を実装・レビュー・merge・検証した。

実装内容:
- Backend Worker registry row の pin/unpin API を追加し、Worker list/detail に `pinned` / `retention_state` を返すようにした。
- Runtime ごとの manual cleanup plan API を追加し、Backend Worker/Workdir/link registry と Runtime observation に基づいて Worker delete / Workdir cleanup/delete candidates を生成するようにした。
- Plan candidate は action kind、reason/blocking reason、linked Worker/Workdir ids、pinned state、cleanliness/file status、running-link status、safe な reclaim bytes placeholder を含む。
- Manual cleanup execution API は expected plan revision/digest を要求し、stale plan を拒否する。
- Pinned Worker/history delete、running-linked Workdir cleanup、dirty/unknown Workdir の confirmation なし discard を拒否する。
- Removed/missing Workdir registry record delete は安全条件のもとで separate action として扱う。
- Runtime-observed/materialized Workdirs は trusted clean evidence なしに `clean` とせず、`unknown` として扱い、normal clean cleanup ではなく explicit discard confirmation path に乗せる。
- Browser UI に Runtime Workdirs cleanup preview/execution と Worker pin/unpin controls を追加した。
- UI では verified-clean cleanup と dirty/unknown-state discard を区別し、raw Runtime materialized path を表示しない。

Review:
- 初回 review は dirty Workdir safety の実データ経路が不十分として `request_changes`。
- `361569a6 fix: require discard confirmation for unknown workdirs` で observed Workdir を `unknown` 扱いにし、unknown/dirty cleanup に explicit confirmation を要求するよう修正。
- focused re-review は `approve`。
- `estimated_reclaim_bytes: None` は safe size source が未実装のため許容と判断された。

Merge / validation:
- Merge commit: `4970a58c merge: manual worker workdir cleanup`。
- Final validation passed:
  - `git diff --check`
  - `cargo test -p yoi-workspace-server --lib`
  - `cargo test -p worker-runtime --features ws-server,fs-store`
  - `cargo check -p yoi`
  - `cd web/workspace && deno task check`
  - `cd web/workspace && deno task test`
  - `yoi ticket doctor`
  - `nix build .#yoi --no-link`
- Validation log: `/run/user/1000/yoi/yoi-orchestrator/bash-output/manual-cleanup-final-validation-1783708592.txt`

---
