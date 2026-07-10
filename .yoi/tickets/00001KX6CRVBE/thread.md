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
