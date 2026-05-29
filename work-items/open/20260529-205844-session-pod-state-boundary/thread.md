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
