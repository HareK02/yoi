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
