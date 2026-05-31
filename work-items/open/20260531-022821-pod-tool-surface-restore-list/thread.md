<!-- event: create author: tickets.sh at: 2026-05-31T02:28:21Z -->

## Created

Created by tickets.sh create.

---

<!-- event: decision author: hare at: 2026-05-31T02:29:17Z -->

## Decision

Decision note:

- `ListVisiblePods` should be folded into `ListPods` rather than kept as a separate tool.
- `InspectPod` is not needed as an LLM-facing tool; `ListPods` should provide enough state/detail, and action should go through restore/send/read/stop tools.
- `AttachOrRestorePod` should become `RestorePod`. The current "attach" branch only observes an already-live socket and returns status/socket data; it does not mean a persistent attachment or comm-registry materialization, so the name is misleading.
- If deeper semantics are needed later, define them explicitly as comm-registration/materialization rather than using "attach" implicitly.


---
