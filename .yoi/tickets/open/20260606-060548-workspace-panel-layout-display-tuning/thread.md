<!-- event: create author: yoi ticket at: 2026-06-06T06:05:48Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: plan author: hare at: 2026-06-06T06:06:30Z -->

## Plan

Created after closing the first-pass workspace orchestration panel implementation.

The first pass deliberately prioritized end-to-end behavior over visual/layout polish. This ticket owns the next tuning pass: row labels, key hints, detail pane content, composer target visibility, concise diagnostics, and optional phase/dependency/timeline display, while preserving existing TUI conventions and the thin ViewModel/action-dispatch boundaries.


---

<!-- event: decision author: hare at: 2026-06-06T08:36:49Z -->

## Decision

User layout direction:

Rows should be column-aligned instead of leading with long identifiers/titles.

- Ticket/action rows: move the long Ticket title to the end. Put short, alignable fields first, such as priority/action/status/phase/id-or-slug, then the title.
- Pod rows: move variable-length Pod id/name to the end. Put short, alignable fields first, such as status/action/role-or-kind, then the Pod id/name.
- Ticket and Pod rows do not need to share exactly the same schema, but each row family should keep stable aligned columns so status/action can be visually scanned.

This should be handled in the layout/display tuning ticket, not by changing backend/action semantics.


---
