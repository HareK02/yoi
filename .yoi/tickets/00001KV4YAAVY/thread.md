<!-- event: create author: "yoi ticket" at: 2026-06-15T06:08:19Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-15T06:37:00Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-15T06:38:25Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Panel Queue により routing が明示的に許可され、Ticket は `queued`。
- Ticket body / thread / relation / OrchestrationPlan / Orchestrator workspace state を確認した。blocking relation はなく、planning に戻す concrete missing information はない。
- 対象は single-Pod conversation View の text-like Item selection/copy に限定され、Panel row selection とは scope が分かれている。
- Tool/non-text Item と copy target は implementation-time decision として許容されており、binding invariants / acceptance criteria / escalation conditions が明確。

Evidence checked:
- Ticket body/thread: requirements、copy target、tool/non-text handling、acceptance criteria、non-goals、related work を確認。
- Ticket relations: `00001KV072V89` と `00001KV10SN02` は related のみで blocker ではない。
- OrchestrationPlan: 既存 record なし。
- Orchestrator workspace: `/home/hare/Projects/yoi/.worktree/orchestration` は clean、`f0de8413` 上。
- Visible Pods: implementation child Pod なし。Intake peer は routing blocker ではない。
- Bounded code map: `crates/tui/src/single_pod.rs`、copy/clipboard abstraction、mouse/selection/key handling tests。

IntentPacket:

Intent:
- single-Pod TUI conversation View で User/System/Assistant など text-like Item の表示テキストを mouse drag で選択し、`y` で copy、`Esc` で clear できるようにする。

Binding decisions / invariants:
- 対象は single-Pod View Item text。Panel Ticket/Pod row selection は変更しない。
- terminal-native text selection preservation は non-goal。View drag は Yoi text selection として扱う。
- selected/copy text、selection state、clipboard diagnostics は Pod history / model context / session log / Ticket records に残さない。
- composer input、scroll、rewind picker、modal/popup、normal key handling と衝突させない。
- bare Panel row mouse selection semantics は regress させない。
- Tool/non-text Item handling と copy target は実装報告または decision comment に明示する。

Requirements / acceptance criteria:
- UserItem / SystemItem / AssistantItem text を drag selection できる。
- drag start/update/release 後に selection state が残り、View 上で highlight される。
- `Esc` で selection clear。
- `y` で selected text を copy し、selection clear。
- text-like Item を跨いだ selection が deterministic separator で copy される。
- non-text/tool item handling decision が明示され、test で固定される。
- copy 成功/失敗が user-visible で、secret-like diagnostics を出さない。
- tests cover coordinate mapping、selection state、multi-item extraction、Esc、copy+clear、non-text/tool handling。

Implementation latitude:
- Existing clipboard abstraction があれば利用。なければ testable な最小 copy path を追加してよい。
- OSC52/system clipboard/internal copy buffer の選択は既存 TUI architecture に合わせる。
- rendering/highlight 表現、selection model の internal shape、separator は実装側判断。ただし deterministic にする。

Escalate if:
- terminal/crossterm event stream で必要な drag/release を識別できない。
- existing rendering model から text coordinate mapping を安全に取れない。
- copy target を追加すると secrets/history/model-context boundary に影響する。
- rewind/modal/composer key handling と共存できない大きな model change が必要。

Validation:
- focused `cargo test -p tui ...` for single_pod selection/copy。
- `cargo check -p tui --all-targets`。
- `cargo fmt --check`。
- `git diff --check`。
- Panel row mouse selection regression test if related mouse plumbing is touched。

Critical risks / reviewer focus:
- selection/copy state の persistence leakage。
- mouse coordinate -> rendered text mapping correctness。
- multi-item extraction ordering/separator。
- composer/scroll/rewind/modal conflict。
- clipboard path safety and testability。
- Panel mouse behavior regression absence。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-15T06:39:16Z from: queued to: inprogress reason: orchestrator_acceptance field: state -->

## State changed

Routing decision と accepted implementation plan を記録済み。blocking relation / unresolved OrchestrationPlan blocker はなく、single-Pod View work は同時に開始する Panel composer work と主対象が異なるため、implementation side effects の前に `queued -> inprogress` acceptance を記録する。

---

<!-- event: implementation_report author: yoi-coder-00001KV4YAAVY at: 2026-06-15T07:02:42Z -->

## Implementation report

Implementation report from Coder Pod `yoi-coder-00001KV4YAAVY`:

Commit:
- `09f5e9d5 feat: add single-pod text selection`

Changed files:
- `crates/tui/src/text_selection.rs`
  - New transient selection state, coordinate mapping, extraction, and focused tests.
- `crates/tui/src/app.rs`
  - Local non-persistent selection state on `App`.
- `crates/tui/src/ui.rs`
  - Selectable history row metadata, highlight rendering, snapshot wiring, selectable/non-text tests.
- `crates/tui/src/single_pod.rs`
  - Drag mouse handling, `Esc` clear, `y` copy+clear, OSC52 copy, tests.
- `crates/tui/src/lib.rs`
  - Module registration.
- `crates/tui/Cargo.toml`, `Cargo.lock`
  - Added direct `base64` dependency for OSC52 clipboard payloads.

Behavior / implementation decisions:
- Selectable Items:
  - `UserMessage`
  - `SystemMessage`
  - `AssistantText`
- Non-text/tool Items:
  - tool calls, thinking, stats, greetings, alerts, turn headers, etc. are not selectable/copyable through this path.
  - extraction skips non-selectable rows.
- Cross-item selection separator:
  - copied text follows rendered row order and preserves selectable blank spacer rows, giving deterministic `\n\n` between adjacent text-like Items.
- Copy target:
  - terminal clipboard via OSC52 (`ESC ] 52 ; c ; base64 BEL`), not OS clipboard APIs.
  - avoids adding platform clipboard runtime behavior.
- Copy diagnostics:
  - actionbar-only sanitized messages:
    - success: `copied selected text to terminal clipboard`
    - failure: `terminal clipboard write failed`
  - no selected content appears in diagnostics.
- Selection persistence:
  - selection state/copied text remains TUI-local only.
  - not appended to Pod history, model context, session logs, Ticket records, memory, or durable state.
- Mouse tracking:
  - single-Pod TUI now enables button-event drag tracking (`?1002h`) but not all-motion tracking (`?1003h`).
  - Panel code was not changed.

Validation reported by coder:
- Passed: `cargo fmt --check`
- Passed: `cargo check -p tui --all-targets`
- Passed: `cargo test -p tui text_selection`
- Passed: `cargo test -p tui single_pod::tests`
- Passed: `cargo test -p tui ui::tests::history_rows_mark_text_items_selectable_and_non_text_unselectable`
- Passed: `git diff --check`

Repository status:
- Child implementation worktree clean after commit.

Residual risks / notes:
- OSC52 clipboard success means sequence written/flushed; actual clipboard acceptance depends on terminal/multiplexer policy.
- No real PTY/E2E validation was run; coverage is focused unit-level mouse/key/selection/render extraction behavior.

---

<!-- event: review author: yoi-reviewer-00001KV4YAAVY at: 2026-06-15T07:11:34Z status: approve -->

## Review: approve

Review result: approve

Evidence checked:
- Reviewed child worktree only:
  - `/home/hare/Projects/yoi/.worktree/00001KV4YAAVY-single-pod-text-selection`
  - diff `368249d6..09f5e9d5`
  - Ticket record/thread in the child worktree
- Read-only validation:
  - Passed: `git diff --check 368249d6..09f5e9d5`

Implementation review:
- TUI-local selection state was added in `crates/tui/src/text_selection.rs` and wired into single-Pod rendering/input.
- Mouse drag maps terminal coordinates through `HistoryViewport` into rendered history rows.
- Drag start/update/release are handled in `single_pod.rs`; selection remains active after release.
- `Esc` clears selection before normal composer handling.
- bare `y` copies only when a selection exists; otherwise composer input remains available.
- Highlighting is applied through `range_for_row` and `highlight_line_selection`.
- Selectable item boundary is explicit:
  - selectable: `UserMessage`, `SystemMessage`, `AssistantText`
  - non-selectable: tool calls, thinking, notices, stats, greetings, etc.
- Tool/non-text rows are marked unselectable and extraction skips unselectable rows.
- Cross-item extraction is deterministic, with documented newline/blank-line behavior.
- Copy uses OSC52; selected text is base64 encoded before writing the OSC52 sequence.
- Copy diagnostics are generic/actionbar-local and do not include selected content.
- Selection state lives in TUI-local `App.text_selection`; no path was found that appends selected/copied text to Pod history, model context, session log, Ticket records, memory, or durable state.
- Mouse capture changes are limited to single-Pod fullscreen setup and avoid all-motion `?1003h`; Panel/multi-pod mouse row selection code was not modified.
- Composer, scroll, rewind picker, completion/key routing, and normal key handling interactions were inspected and no blocker found.

Coverage reviewed:
- coordinate mapping
- drag update/release state
- multi-row / separator extraction
- non-selectable row skipping
- Esc clear
- copy helper + clear
- UI selectable/non-selectable marking
- mouse capture mode avoiding all-motion tracking

Non-blocking notes:
- No PTY/E2E validation was run; terminal-specific OSC52/mouse behavior still depends on real terminal behavior.
- OSC52 cannot acknowledge terminal clipboard acceptance; implementation can only report write/flush success/failure.
- There is no explicit byte-size cap on OSC52 payload, but selection is bounded by visible rendered viewport rather than arbitrary full history traversal. Future hard cap/error notice would be reasonable hardening.
- A full fixture test constructing an actual `Block::ToolCall` could strengthen coverage, but the non-text path is explicit and source-inspected.

Validation not rerun by reviewer:
- Cargo validation was not rerun because review scope was read-only and Cargo writes build artifacts. Coder-reported validation was inspected as evidence.

Conclusion:
- Approved. No blocker remains.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-15T07:15:54Z -->

## Implementation report

Integration report:

Merged implementation branch into orchestration branch:
- branch: `impl/00001KV4YAAVY-single-pod-text-selection`
- implementation commit: `09f5e9d5 feat: add single-pod text selection`
- merge commit: `3fa52f2c merge: single-pod text selection`

Reviewer result:
- `approve` recorded from `yoi-reviewer-00001KV4YAAVY`.

Additional integration fix:
- Because the implementation added a direct `base64` dependency to `crates/tui/Cargo.toml` and updated `Cargo.lock`, `nix build .#yoi --no-link` first failed with stale `package.nix` `cargoHash`.
- Updated `package.nix` `cargoHash` to the new fixed-output vendor hash:
  - `sha256-pIDYnbBs3U8Z3IndgH10rirv8/IdFv1WlgwpCbKXy+M=`

Validation in Orchestrator worktree `/home/hare/Projects/yoi/.worktree/orchestration`:
- Passed: `cargo fmt --check`
- Passed: `cargo check -p tui --all-targets`
- Passed: `cargo test -p tui text_selection`
  - 4 passed
- Passed: `cargo test -p tui single_pod::tests`
  - 38 passed
- Passed: `cargo test -p tui ui::tests::history_rows_mark_text_items_selectable_and_non_text_unselectable`
  - 1 passed
- Passed: `git diff --check`
- Passed after hash update: `nix build .#yoi --no-link`

Notes:
- OSC52 clipboard acceptance remains terminal/multiplexer dependent; implementation reports write/flush success/failure only.
- No PTY/E2E validation was run for real terminal selection/copy behavior; focused unit coverage and reviewer inspection covered selection/copy state, extraction, and persistence boundaries.
- Orchestrator worktree is clean apart from the pending `package.nix`/Ticket integration commit at the time of recording.

Cleanup planned:
- Stop related coder/reviewer Pods.
- Remove only child implementation worktree/branch for this Ticket.

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-15T07:15:54Z from: inprogress to: done reason: merged_validated field: state -->

## State changed

Reviewer approved, implementation branch merged into the orchestration branch, integration validation passed including Nix package hash refresh, and focused validation passed in the Orchestrator worktree. Marking Ticket done in the orchestration branch.

---

<!-- event: state_changed author: hare at: 2026-06-15T14:59:40Z from: done to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-06-15T14:59:40Z status: closed -->

## 完了

Ticket `00001KV4YAAVY` (`single-Pod View Item text をマウスドラッグで選択・コピーできるようにする`) はすでに `state: done` に到達していたため、workspace Panel から close しました。

この Close action によって、実装作業、state 変更、Orchestrator/Companion launch、worker invocation は開始されていません。


---
