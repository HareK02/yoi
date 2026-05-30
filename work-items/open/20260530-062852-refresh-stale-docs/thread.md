<!-- event: create author: tickets.sh at: 2026-05-30T06:28:52Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-05-30T06:29:24Z -->

## Plan

Implementation plan:

1. Read the current target docs and authoritative code/type definitions.
2. Update high-level docs in-place, keeping edits scoped to stale claims rather than broad prose rewrites.
3. Mark superseded plan docs clearly when replacing them would be larger than useful.
4. Validate with `./tickets.sh doctor` and `git diff --check`.
5. Review the resulting diff against the audit findings before closing.


---
