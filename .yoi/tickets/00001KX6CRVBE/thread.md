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
