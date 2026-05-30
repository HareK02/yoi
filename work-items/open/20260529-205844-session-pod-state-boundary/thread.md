<!-- event: create author: tickets.sh at: 2026-05-29T20:58:44Z -->

## Created

Created by tickets.sh create.

---

<!-- event: review author: review-session-pod-state-boundary at: 2026-05-29T23:04:00Z -->

## External review

Initial review found blocking issues in restore reconciliation: missing child allocations left stale runtime deny entries, and reconciliation was not enforced at the public restore boundary. The coder fixed these in commit `d2e8087`; second review approved the implementation.

Artifacts:
- `artifacts/review.md`
- `artifacts/review-r2.md`

---

<!-- event: fix author: insomnia at: 2026-05-30T00:08:00Z -->

## Parent-side validation fix

After merging the approved implementation, post-merge validation failed on `cargo test -p pod --test controller_test empty_turn_pause_rolls_back_and_snapshot_does_not_restore_input`.

The parent took over the stopped/failed handoff and fixed the adjacent turn-control regression directly on main: cancellation received immediately after the controller accepts a run was being lost before the worker reached its first stream event wait, so empty turns could hang instead of rolling back. The fix preserves idle stale-cancel cleanup at the controller boundary and makes first-event waiting cancellation-aware.

While investigating the child Pod's `context_length_exceeded` ping failure, the parent also fixed provider terminal stream errors so `Event::Error` is not only a live TUI event: terminal provider errors now fail the worker turn and persist `RunErrored` instead of allowing an empty `RunCompleted::Finished`.

---
