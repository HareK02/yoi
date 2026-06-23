<!-- event: create author: ticket-intake at: 2026-06-23T05:40:56Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-23T06:08:42Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-23T06:11:11Z -->

## Decision

Routing decision: `implementation_ready_parallel`

Reason:
- Ticket body has concrete Dashboard render targets and acceptance criteria。
- No relations / blockers / orchestration plan records exist。
- Active work `00001KVSEBF56` (protocol TS generation) and `00001KVSGFM65` (Workspace web Kanban review) are separate from TUI Dashboard render code。
- Orchestrator worktree is clean on `orchestration` at `0e635ba6`; target worktree / branch is not present。
- Bounded code map confirms relevant functions in `crates/tui/src/dashboard/render.rs`: `draw_title`, `target_status_line`, `actionbar_left_text`, `actionbar_right_text`, `draw_actionbar`, and tests in `crates/tui/src/dashboard/tests.rs`。

IntentPacket:

Intent:
- Remove redundant Dashboard key hints and selected-row textual status while preserving actual row selection visibility and keyboard behavior。

Binding decisions / invariants:
- This Ticket only changes Dashboard display cleanup。
- Console / single-Pod TUI key hints are out of scope。
- Do not remove list row selection markers / highlighting / keyboard navigation visibility。
- Do not reintroduce direct selected-Pod send。
- Companion lifecycle / Orchestrator lifecycle / Ticket workflow semantics must not change。
- Existing Dashboard actions must remain: blank Enter row action, text Enter composer-target action, Tab target switching, Esc clear selection, Pod open/attach, Ticket Intake/Queue flows。

Requirements / acceptance criteria:
- Top title line no longer shows key hint guidance such as `Row selection`, `blank Enter`, `Tab target`。
- Composer bottom actionbar no longer shows always-on key hint groups, or is minimized to non-redundant notices only。
- Selected Ticket / selected Pod / selected Intake Pod / no row selected textual status line is removed or no longer recognized as redundant selected-row status。
- List still visibly indicates current selected row。
- Related render/unit tests are updated to the new display specification。

Implementation latitude:
- `target_status_line` layout row may be removed, made empty/minimal, or repurposed only if it does not show redundant selected-row status。
- Actionbar may be fully removed, notice-only, or otherwise minimized depending on layout/tests。
- Keep row markers like `▶` and selected styling unless explicitly escalated。
- If layout row removal creates resize/hitbox complications, choose minimal empty/notice behavior and document the tradeoff。

Escalate if:
- Removing status/actionbar rows makes current composer target/action ambiguous enough to require alternative UI。
- Row selection marker/visibility also needs removal。
- Layout changes cause terminal resize / hitbox / mouse selection side effects that require broader Dashboard layout refactor。

Validation plan:
- `cargo fmt --check`
- `cargo test -p tui dashboard --lib`
- `cargo test -p tui workspace_panel --lib` if affected by shared tests/helpers。
- `git diff --check`
- `cargo run -p yoi -- ticket doctor`
- Manual/PTY `yoi panel` check if practical, but focused render tests are primary。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-23T06:11:22Z from: queued to: inprogress reason: human_authorized_unblocked_dashboard_hint_cleanup field: state -->

## State changed

Human authorized queue routing from Workspace Dashboard. Ticket has concrete Dashboard render acceptance criteria and no recorded blockers, so Orchestrator accepts implementation.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-23T06:12:36Z -->

## Implementation report

Implementation start report:

- Created child implementation worktree:
  - `/home/hare/Projects/yoi/.worktree/00001KVSFXY88-dashboard-hint-cleanup`
- Created branch:
  - `impl/00001KVSFXY88-dashboard-hint-cleanup`
- Base commit:
  - `3bfd1ca0 ticket: accept dashboard hint cleanup`
- Confirmed tracked Ticket project records are visible in the child worktree。
- Confirmed `.yoi/memory` has no tracked/untracked entries in the child worktree check。
- Spawned sibling Coder Pod:
  - `yoi-coder-00001KVSFXY88`
- Coder scope:
  - read: `/home/hare/Projects/yoi`
  - write: `/home/hare/Projects/yoi/.worktree/00001KVSFXY88-dashboard-hint-cleanup`

Parallelization note:
- Active Workspace web / protocol work is separate from TUI Dashboard rendering. This Ticket should stay limited to `crates/tui/src/dashboard/*` unless tests reveal a narrow shared helper impact。

Next action:
- Wait for Coder implementation report, then spawn Reviewer read-only for the implementation diff。

---
