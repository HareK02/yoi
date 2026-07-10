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
