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
