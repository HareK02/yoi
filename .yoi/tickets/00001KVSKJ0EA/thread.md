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

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-23T07:15:44Z -->

## Implementation report

Coder implementation report received from `yoi-coder-00001KVSKJ0EA`.

Implementation commit:
- `5c242d96 fix: keep dashboard row selection explicit`

Changed areas:
- `crates/tui/src/dashboard/mod.rs`
  - Reload/snapshot reconciliation preserves only an explicitly selected visible row。
  - Initial/reload visibility correction no longer auto-selects a row。
  - Stale/disappeared selections fall back to `None`。
  - `Esc` clear state remains no-selection across reload。
  - Keyboard navigation from no-selection explicitly creates selection on first visible row。
  - `list.selected_name` stays aligned with actual Pod row selection and clears for non-Pod/no-selection states。
- `crates/tui/src/dashboard/tests.rs`
  - Added focused coverage for initial visible rows with no selected row。
  - Added `Esc` clear surviving reload while preserving composer draft。
  - Added no-selection + `TicketIntake` submit routing to global/new Intake。
  - Added keyboard navigation creating selection explicitly。
  - Added reload fallback to no-selection when selected row disappears。
  - Updated existing Dashboard tests to explicitly select rows where previous behavior depended on auto-selection。

Coder validation reported:
- `cargo fmt --check`: passed
- `cargo test -q -p tui workspace_panel`: passed
- `cargo test -q -p tui dashboard`: passed
- `git diff --check`: passed
- `cargo run -p yoi -- ticket doctor`: passed (`doctor: ok`)

Orchestrator evidence checked:
- Worktree clean on `impl/00001KVSKJ0EA-dashboard-no-auto-selection` at `5c242d96`。
- Diff `4cda83b7..HEAD` touches expected Dashboard model/tests only。
- `git diff --check 4cda83b7..HEAD`: passed。

Next action:
- Route to read-only Reviewer Pod for acceptance review。

---

<!-- event: comment author: yoi-orchestrator at: 2026-06-23T07:16:33Z -->

## Comment

Review routing:

Spawned read-only Reviewer Pod:
- `yoi-reviewer-00001KVSKJ0EA-r1`

Review target:
- Worktree: `/home/hare/Projects/yoi/.worktree/00001KVSKJ0EA-dashboard-no-auto-selection`
- Branch: `impl/00001KVSKJ0EA-dashboard-no-auto-selection`
- Baseline: `4cda83b7`
- Implementation commit: `5c242d96 fix: keep dashboard row selection explicit`
- Diff: `4cda83b7..5c242d96`

Reviewer scope:
- read-only `/home/hare/Projects/yoi`

Reviewer focus:
- Initial/reload no-auto-selection semantics。
- Esc clear persistence and composer draft preservation。
- Explicit keyboard/mouse selection still works。
- Stale selection falls back to none。
- No-selection TicketIntake composer routing。
- `list.selected_name` sync and no stale Pod selection。
- No row-action / workflow semantic regressions。

Orchestrator will wait for reviewer verdict before integration。

---

<!-- event: review author: yoi-orchestrator at: 2026-06-23T07:23:02Z status: approve -->

## Review: approve

Reviewer Pod `yoi-reviewer-00001KVSKJ0EA-r1` approved the implementation.

Reviewed target:
- Worktree: `/home/hare/Projects/yoi/.worktree/00001KVSKJ0EA-dashboard-no-auto-selection`
- Branch: `impl/00001KVSKJ0EA-dashboard-no-auto-selection`
- Implementation: `5c242d96 fix: keep dashboard row selection explicit`
- Diff: `4cda83b7..5c242d96`

Reviewer evidence:
- 初期表示 / reload path は row selection を自動生成しない。
- `selected_row = None` は `Esc` 後も reload/snapshot reconciliation を跨いで維持される。
- 明示的な keyboard navigation は no-selection から selection を作成できる。
- 明示的 selection は row が visible な間は維持される。
- 消えた / stale row selection は `None` へ落ちる。
- `list.selected_name` は no-selection / non-Pod selection で clear され、実際の Pod row selection のみで設定される。
- no-selection + `TicketIntake` submit は global/new Intake launch へ進む。
- mouse click は selection-only のまま。
- 差分は Dashboard selection logic と focused tests に限定され、layout / render / workflow semantic の不要変更はない。

Blocking issues:
- なし。

Non-blocking concerns:
- なし。

Reviewer validation:
- `cargo fmt --check`: passed
- `git diff --check 4cda83b7..HEAD`: passed
- `cargo test -q -p tui workspace_panel`: passed (`27 passed`)
- `cargo test -q -p tui dashboard`: passed (`111 passed`)
- `cargo run -q -p yoi -- ticket doctor`: passed (`doctor: ok`)
- `cargo test -q -p tui`: passed (`372 passed`)

Worktree status:
- clean at `5c242d96`。

---
