<!-- event: create author: tickets.sh at: 2026-05-29T23:54:08Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-05-30T00:23:45Z -->

## Plan

Use the worktree + sibling coder/reviewer flow for this ticket.

Intent packet summary:
- Preserve `session.record_event_trace` through profile resolution and spawned-Pod manifest construction when the parent/profile enables it.
- Keep event trace opt-in; do not globally enable it by default.
- Treat `.trace.jsonl` as normalized provider stream/lifecycle diagnostics, not byte-for-byte raw SSE.

Implementation direction:
- Trace where `record_event_trace` is carried in profile-resolved manifests and where `SpawnPod` constructs hidden child spawn config.
- Fix the narrow point where the setting is dropped/defaulted.
- Add focused tests for profile/spawn config preserving `record_event_trace = true`.
- Add a small documentation/comment clarification for trace sidecar semantics.

Validation target:
- Focused manifest/pod tests covering the changed path.
- `./tickets.sh doctor` and `git diff --check`.


---
