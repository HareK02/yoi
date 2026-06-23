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

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-23T06:28:12Z -->

## Implementation report

Coder implementation report received from `yoi-coder-00001KVSFXY88`.

Implementation commit:
- `03ad525f tui: trim dashboard redundant hints`

Changed areas:
- `crates/tui/src/dashboard/render.rs`
  - Removed top title key-hint guidance。
  - Made selected-row/composer target status line blank。
  - Minimized actionbar to notices/diagnostic-only text。
  - Preserved row marker/highlighting rendering。
- `crates/tui/src/dashboard/mod.rs`
  - Removed now-unused selected-row display-status helper。
- `crates/tui/src/dashboard/tests.rs`
  - Updated render/unit expectations for no redundant hints/status。
  - Added title guidance cleanup assertion。
  - Kept assertions that selected rows remain visibly marked。

Coder validation reported:
- `cargo fmt --check`: passed
- `cargo test -p tui dashboard --lib`: passed
- `cargo test -p tui workspace_panel --lib`: passed
- `git diff --check`: passed
- `cargo run -p yoi -- ticket doctor`: passed

Manual/PTY `yoi panel` check:
- Not run; focused render/unit tests were used as primary validation。

Orchestrator evidence checked:
- Worktree clean on `impl/00001KVSFXY88-dashboard-hint-cleanup`。
- HEAD is `03ad525f`。
- Diff `3bfd1ca0..HEAD` touches expected dashboard render/module/tests files only。
- `git diff --check 3bfd1ca0..HEAD`: passed。

Next action:
- Route to read-only Reviewer Pod for acceptance review。

---

<!-- event: comment author: yoi-orchestrator at: 2026-06-23T06:28:54Z -->

## Comment

Review routing:

Spawned read-only Reviewer Pod:
- `yoi-reviewer-00001KVSFXY88-r1`

Review target:
- Worktree: `/home/hare/Projects/yoi/.worktree/00001KVSFXY88-dashboard-hint-cleanup`
- Branch: `impl/00001KVSFXY88-dashboard-hint-cleanup`
- Baseline: `3bfd1ca0`
- Implementation commit: `03ad525f tui: trim dashboard redundant hints`
- Diff: `3bfd1ca0..03ad525f`

Reviewer scope:
- read-only `/home/hare/Projects/yoi`

Reviewer focus:
- Removal of redundant title/actionbar key hints and selected-row textual status。
- Preservation of visible row selection marker/highlighting。
- No keyboard behavior/action semantics changes。
- Dashboard render tests updated to new display contract。

Orchestrator will wait for reviewer verdict before integration。

---

<!-- event: review author: yoi-reviewer-00001KVSFXY88-r1 at: 2026-06-23T06:31:21Z status: approve -->

## Review: approve

Verdict: `approve`

確認範囲:
- Ticket authority:
  - `.yoi/tickets/00001KVSFXY88/item.md`
  - `.yoi/tickets/00001KVSFXY88/thread.md`
- Diff `3bfd1ca0..03ad525f`:
  - `crates/tui/src/dashboard/render.rs`
  - `crates/tui/src/dashboard/mod.rs`
  - `crates/tui/src/dashboard/tests.rs`

Evidence:
- Top title guidance was removed; `title_line` now keeps only `workspace dashboard` plus companion/orchestrator status。
- `target_status_line` now renders blank, removing selected Ticket / Pod / Intake / no-row textual status。
- Actionbar is reduced to sending/refreshing/notices/diagnostic text; always-on key hint groups are gone。
- Row marker/highlighting paths remain; tests assert selected row marker strings such as `▶ ready`, `▶ queued`, and `  ▶ live`。
- Keyboard/action behavior was not altered in the diff; removed helpers were display-only and no remaining references exist。
- No Console/single-Pod TUI files changed。
- No direct selected-Pod send was reintroduced; existing dashboard behavior tests still pass。

Blocking issues:
- None。

Non-blocking concerns:
- Manual `yoi panel` PTY visual check was not run; focused render/unit coverage was used as primary validation, as suggested。

Reviewer validation:
- `cargo fmt --check`: passed
- `git diff --check 3bfd1ca0..HEAD`: passed
- `cargo test -p tui dashboard --lib`: passed (`107 passed`)
- `cargo test -p tui workspace_panel --lib`: passed (`23 passed`)
- `cargo run -p yoi -- ticket doctor`: passed (`doctor: ok`)

Worktree status:
- Clean on `impl/00001KVSFXY88-dashboard-hint-cleanup` at `03ad525f`。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-23T06:32:25Z -->

## Implementation report

Reviewer r1 approvalを受けて、実装ブランチを Orchestrator worktree の `orchestration` branch に統合した。

統合:
- Merge commit: `5abf16f9 merge: dashboard hint cleanup`
- Merged branch: `impl/00001KVSFXY88-dashboard-hint-cleanup`
- Implementation commit:
  - `03ad525f tui: trim dashboard redundant hints`

Orchestrator worktree validation:
- `cargo fmt --check`: passed
- `git diff --check HEAD^1..HEAD`: passed
- `cargo test -p tui dashboard --lib`: passed (`107 passed`)
- `cargo test -p tui workspace_panel --lib`: passed (`23 passed`)
- `cargo run -p yoi -- ticket doctor`: passed (`doctor: ok`)

Final Orchestrator worktree status after validation is clean on `orchestration` at `5abf16f9`。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-23T06:32:37Z from: inprogress to: done reason: implementation_merged_and_validated field: state -->

## State changed

Reviewer approval、Orchestrator worktree への統合、focused Dashboard / workspace_panel tests、Ticket doctor が完了したため `done` に遷移する。

---

<!-- event: state_changed author: hare at: 2026-06-23T06:32:49Z from: done to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-06-23T06:32:49Z status: closed -->

## 完了

TUI Dashboard の冗長な key hints と selected-row textual status display を削除し、Orchestrator worktree の `orchestration` branch に統合した。

主な成果:
- Top title line から `Row selection`, `blank Enter`, `Tab target` などの key hint guidance を削除。
- Selected Ticket / Pod / Intake / no-row selected textual status line を blank にした。
- Composer bottom actionbar を always-on key hints ではなく notices / diagnostics only に最小化。
- Row marker / highlighting rendering は維持。
- Keyboard/action behavior は変更していない。
- Unused selected-row display-status helper を削除。
- Dashboard render/unit tests を新 display contract に更新し、selected row marker の visibility assertion は維持。

統合・検証:
- Merge commit: `5abf16f9 merge: dashboard hint cleanup`
- Implementation commit: `03ad525f tui: trim dashboard redundant hints`
- Reviewer final verdict: approve
- Validation passed: `cargo fmt --check`, `git diff --check HEAD^1..HEAD`, `cargo test -p tui dashboard --lib`, `cargo test -p tui workspace_panel --lib`, and `cargo run -p yoi -- ticket doctor`。

範囲外:
- Console / single-Pod TUI hints は変更していない。
- Manual/PTY `yoi panel` visual check は実施していない。Focused render/unit tests を主 validation とした。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-23T06:33:21Z -->

## Implementation report

Post-close cleanup completed。

- Stopped child Pods and reclaimed scope:
  - `yoi-coder-00001KVSFXY88`
  - `yoi-reviewer-00001KVSFXY88-r1`
- Removed implementation worktree:
  - `/home/hare/Projects/yoi/.worktree/00001KVSFXY88-dashboard-hint-cleanup`
- Deleted implementation branch:
  - `impl/00001KVSFXY88-dashboard-hint-cleanup`
- Orchestrator worktree remains clean on `orchestration` at `10866666`。

Root/original workspace was not used for merge/validation/cleanup operations。

---
