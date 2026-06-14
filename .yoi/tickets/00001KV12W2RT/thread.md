<!-- event: create author: "yoi ticket" at: 2026-06-13T18:10:57Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-14T06:08:41Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-06-14T06:10:02Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Ticket is queued and specifies a bounded Workspace Panel row layout change: two-line Ticket rows separating canonical state from derived gate/action/reason.
- Relation checks show related examples only, no blockers. `00001KV10SN02` E2E mouse/wheel coverage is done and can be updated as needed.
- Risk is TUI layout/mouse selection; acceptance criteria are concrete.

IntentPacket:
- Render Ticket rows as two visual lines, use `|` selected grouping marker, show canonical state/title on line 1 and id/gate/action/reason on line 2, and adjust selection/mouse/scrolling logic accordingly.

Binding invariants:
- Do not add persisted `waiting` state or change Ticket lifecycle.
- Normal relation waits are gate information, not error/human-reply styling.
- Mouse click on either visual line selects the same logical Ticket and does not dispatch actions.

Validation:
- focused TUI row/layout/mouse tests, affected E2E if practical, `cargo build -p yoi`, `git diff --check`, `cargo fmt --check`; `nix build .#yoi` if feasible.

---

<!-- event: state_changed author: orchestrator at: 2026-06-14T06:10:45Z from: queued to: inprogress reason: orchestrator_acceptance_unblocked field: state -->

## State changed

Ticket evidence, related records, orchestration plan, and clean workspace state were checked. No blockers remain; accept for implementation before worktree/spawn side effects.

---

<!-- event: implementation_report author: hare at: 2026-06-14T06:26:55Z -->

## Implementation report

Implemented Workspace Panel Ticket row rendering as two visual lines.

Changes:
- Ticket action rows now render a state/title line and a separate detail line containing Ticket id, gate, action, and reason.
- Selection stays logical per Ticket row: selected Ticket visual lines use the same `|` marker, and contiguous hit boxes for the same Ticket are grouped so mouse clicks on either line select the same Ticket without dispatching actions.
- Relation blockers are shown as gate/wait information with queue disabled wording and `Wait` action rather than as blocked/error/human-reply styling; no persisted `waiting` lifecycle state was added.
- Updated focused row layout, waiting-gate, and mouse hit-testing tests.

Validation:
- PASS: `cargo test -p tui panel_ticket --lib`
- PASS: `cargo test -p tui row_hit_testing --lib`
- PASS: `cargo test -p tui mouse_click --lib`
- PASS: `cargo test -p tui ready_ticket_with_waiting_gate --lib`
- PASS: `cargo test -p tui workspace_panel_marks_ready_ticket_with_unresolved_relation_waiting_gate --lib`
- PASS: `cargo test -p yoi-e2e --features e2e --test panel -- --nocapture`
- PASS: `cargo build -p yoi`
- PASS: `cargo fmt --check`
- PASS: `git diff --check`
- NOTE: `cargo test -p tui --lib` was also attempted and failed in pre-existing/unrelated tests: `multi_pod::tests::orchestrator_launch_context_uses_orchestration_root_for_runtime_workspace`, `spawn::tests::profile_choices_include_builtin_and_project_default_marker`, and `spawn::tests::profile_choices_use_project_registry_default`. The failures assert unrelated orchestration/profile defaults and are outside this Ticket's row/layout/mouse changes.
- NOT RUN: `nix build .#yoi` (skipped as nonessential here; it may create root-level build output symlinks and is heavier than the scoped TUI/panel validation).

Risks:
- Ticket rows now consume two terminal lines, so very short panel areas may display fewer logical Ticket rows before clipping.


---
