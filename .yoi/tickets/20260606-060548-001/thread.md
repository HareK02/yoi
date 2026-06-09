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

<!-- event: plan author: hare at: 2026-06-06T09:32:01Z -->

## Plan

Preflight result: `implementation-ready` as a focused layout/display pass.

Implementation should rework panel row rendering into aligned columns with short comparable fields first and long variable text last:
- Ticket/action rows: move Ticket title to the end; align priority/action/status/phase/id-or-slug first.
- Pod rows: move Pod id/name to the end; align status/action/kind first.

This should not change Ticket backend semantics, action dispatch, Orchestrator lifecycle, Intake handoff, no-Ticket behavior, or the thin ViewModel boundary.

Detailed delegation intent is recorded in `artifacts/delegation-intent.md`.


---

<!-- event: review author: hare at: 2026-06-06T21:16:52Z status: approve -->

## Review: approve

External reviewer approved the implementation with no requested changes.

Review summary:
- Ticket/action rows use stable fixed-width columns first and place the variable Ticket title last.
- Pod rows use stable fixed-width columns first and place the variable Pod name/id last.
- Long titles/names truncate after stable columns and do not shift status/action alignment.
- Color/style semantics are preserved closely.
- Changes are scoped to rendering/helpers/tests; no Ticket action dispatch, Intake launch, Orchestrator lifecycle, Pod open/direct-send, composer target, authority, or I/O semantics changed.
- No `--multi` route was reintroduced.
- Tests cover row rendering, alignment, and truncation.


---

<!-- event: close author: hare at: 2026-06-06T21:16:52Z status: closed -->

## Closed

Implemented workspace panel aligned row layout.

Final Ticket/action row schema:

```text
<marker><priority> <action> <status> <phase> <slug-or-id> <title>
```

Column behavior:
- marker: width 2;
- priority: width 11;
- action: width 7;
- status: width 24;
- phase: width 12;
- slug-or-id: width 32;
- title: flexible remaining width.

Fixed columns are padded/truncated before the title. The long Ticket title is last and truncates with `…` when needed.

Final Pod row schema:

```text
<marker><status> <action> <kind> <pod-name>
```

Column behavior:
- marker: width 2;
- status: width 18;
- action: width 8;
- kind: width 3, currently `pod`;
- pod name: flexible remaining width.

Fixed Pod columns are padded/truncated before the Pod name. Long Pod names no longer shift status/action alignment.

Tests added/updated:
- `panel_ticket_rows_use_aligned_columns_before_title`
- `panel_ticket_title_truncates_after_stable_columns`
- `panel_pod_rows_use_aligned_columns_before_pod_name`
- `panel_pod_name_truncates_after_status_action_and_kind`
- adjusted action-before-pod ordering test to locate Ticket rows by slug because title is no longer the leading field.

Validation after merge:
- `cargo test -p tui panel_`
- `cargo test -p tui workspace_panel`
- `cargo test -p tui multi_pod`
- `cargo test -p yoi panel`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `git diff --check HEAD~1..HEAD`
- `cargo build -p yoi`
- `target/debug/yoi ticket doctor`
- `nix build .#yoi --no-link --print-out-paths`

External review approved with no requested changes.

Remaining UX/display tuning:
- Broader detail-pane/timeline/copy tuning remains optional follow-up; this ticket intentionally stayed focused on aligned row columns.


---
