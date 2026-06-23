<!-- event: create author: LocalTicketBackend at: 2026-06-23T06:44:20Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: ticket-intake at: 2026-06-23T06:46:36Z -->

## Intake summary

ユーザー合意により ready 化。Ticket は `implementation_ready` で、初回 `yoi panel` 表示時から未選択、`Esc` 後の no-selection を reload が復活させない、未選択 + `TicketIntake` composer submit を global/new Intake に流す、という要件・受け入れ条件・validation が明記されている。未決定点なし。

---

<!-- event: state_changed author: ticket-intake at: 2026-06-23T06:46:36Z from: planning to: ready reason: user_requested_ready field: state -->

## State changed

ユーザーから `readyにして` の明示指示があり、対象 Ticket の item/thread を確認した。要件・受け入れ条件・risk flags・validation が揃っており、Orchestrator が routing 可能なため `ready` に遷移する。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-23T06:47:17Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-23T06:49:08Z -->

## Decision

Routing decision: `implementation_ready_parallel`

Reason:
- Ticket body has concrete Dashboard selection semantics and focused test requirements。
- No relations / blockers / orchestration plan records exist。
- Active queued Workspace identity Ticket is backend/workspace-server focused and separate from TUI Dashboard row-selection code, so parallel implementation is acceptable。
- Orchestrator worktree is clean on `orchestration` at `13e76d35`; target worktree / branch is not present。
- Code map from Ticket body points to `crates/tui/src/dashboard/mod.rs`, `tests.rs`, and `render.rs`。

IntentPacket:

Intent:
- Treat no-selection as a first-class Dashboard UX state: initial panel display and reload/background refresh must not auto-select rows, while explicit keyboard/mouse selection still works。

Binding decisions / invariants:
- Initial `yoi panel` display should have `selected_row = None` even when rows are visible。
- `Esc` clear must persist across reload/snapshot refresh; reload must not recreate selection after explicit no-selection。
- If a user explicitly selected a row and it remains visible after reload, that selection may be preserved。
- If selected row disappears after reload, fall back safely to no-selection。
- Composer draft must survive selection clear/reload。
- Existing row actions, queue/close/open operations, Ticket workflow state semantics, and row-click-is-selection-only policy must remain unchanged。
- No broad Dashboard layout redesign in this Ticket。

Requirements / acceptance criteria:
- Initial Dashboard state has no selected row when rows exist。
- Esc clear -> reload completion -> still no selected row。
- No-selection + `TicketIntake` composer submit routes to global/new Intake, not selected Ticket refinement。
- Keyboard navigation or mouse click can explicitly create selection again。
- Focused tests cover the above semantics。

Implementation latitude:
- Use an explicit no-selection flag or refine selection visibility correction conditions。
- The key is to make selection creation happen only from explicit user navigation/click/action paths, not from reload/init visibility correction。

Escalate if:
- Initial no-selection substantially breaks keyboard-only navigation or blank Enter action contract。
- Existing selection visibility correction has safety roles that cannot be preserved with a local change。
- Real terminal / PTY behavior is required beyond focused tests。

Validation plan:
- `cargo fmt --check`
- `cargo test -q -p tui workspace_panel`
- Dashboard focused tests as needed。
- `git diff --check`
- `cargo run -p yoi -- ticket doctor`。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-23T06:49:18Z from: queued to: inprogress reason: human_authorized_unblocked_dashboard_no_auto_selection field: state -->

## State changed

Human authorized queue routing from Workspace Dashboard. Ticket has concrete Dashboard selection semantics and no recorded blockers, so Orchestrator accepts implementation.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-23T06:51:32Z -->

## Implementation report

Implementation start report:

- Created child implementation worktree:
  - `/home/hare/Projects/yoi/.worktree/00001KVSKJ0EA-dashboard-no-auto-selection`
- Created branch:
  - `impl/00001KVSKJ0EA-dashboard-no-auto-selection`
- Base commit:
  - `4cda83b7 ticket: accept workspace identity and selection work`
- Confirmed tracked Ticket project records are visible in the child worktree。
- Confirmed `.yoi/memory` has no tracked/untracked entries in the child worktree check。
- Spawned sibling Coder Pod:
  - `yoi-coder-00001KVSKJ0EA`
- Coder scope:
  - read: `/home/hare/Projects/yoi`
  - write: `/home/hare/Projects/yoi/.worktree/00001KVSKJ0EA-dashboard-no-auto-selection`

Parallelization note:
- `00001KVSKGDYS` is active separately and targets Workspace backend identity persistence. This Ticket should stay limited to TUI Dashboard selection semantics。

Next action:
- Wait for Coder implementation report, then spawn Reviewer read-only for the implementation diff。

---
