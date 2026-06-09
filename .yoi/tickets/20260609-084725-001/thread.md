<!-- event: create author: intake at: 2026-06-09T08:47:25Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: intake at: 2026-06-09T08:47:33Z -->

## Intake summary

Panel 表示・操作改善 Ticket を作成した。`Ctrl+T` はトップ行/actionbar/help text から削除し、target switching は `Tab` に変更する。`Tab` は completion が active な場合 completion を優先し、completion がない Panel 状態では target switching として扱う。Panel composer は通常チャット UI composer と低レベル `InputBuffer` は共有しているが key handling が分岐しているため、Panel 専用の ad-hoc 実装を増やさず、composer editing keymap/action model を共有または抽象化して互換性を維持する。

---

<!-- event: state_changed author: intake at: 2026-06-09T08:47:33Z from: planning to: ready reason: intake_ready field: state -->

## State changed

要件・binding decisions・受け入れ条件が揃ったため、Orchestrator が routing できる ready 状態にする。

---

<!-- event: decision author: intake at: 2026-06-09T08:54:26Z -->

## Decision

## Refinement: composer status line and mixed operation model

User clarified an additional UX issue in the Panel composer area:

- The line above the composer currently showing text like `composer Companion · ticket ready · Edit` is redundant and does not make the available action clear.
- Panel currently mixes two interaction models in the same visual/control area:
  - when the composer is empty, row selection with Up/Down plus Enter dispatches the selected Ticket action/open behavior;
  - when the composer has text, Enter sends the text to the selected composer target, either Companion or new Ticket Intake.
- The implementation should make this separation visible and understandable instead of presenting row action state and composer target/edit state as one ambiguous status line.

Additional requirements:

- Rework the composer-adjacent status line so it does not redundantly restate `composer`, target name, Ticket state, and `Edit` without explaining the action.
- Clearly distinguish selected-row action context from text-composer target context.
- The UI should make the empty-composer Enter behavior and non-empty-composer Enter behavior understandable at a glance.
- If the implementation keeps both behaviors, the display should communicate the mode/priority without making it look like the selected Ticket is the composer destination.
- If the implementation introduces a more explicit focus/mode model, it must preserve the binding decisions already recorded: no bare letter shortcuts, `Tab` target switching with completion priority, and shared composer key handling with the normal chat UI.

Reviewer focus:

- Verify that the final Panel display does not conflate selected Ticket actions with Companion/Intake text submission.
- Verify that the composer status/help text answers “what will Enter do now?” rather than only listing internal state labels.

---

<!-- event: decision author: intake at: 2026-06-09T08:59:44Z -->

## Decision

## Refinement: selected Ticket while composer has text

Clarified current behavior and resulting UX invariant:

- In the current Panel implementation, a selected Ticket only drives `Enter` actions when the composer is blank.
- If the composer has text and the target is `Companion`, `Enter` sends the text to the workspace Companion; the selected Ticket is not used as the message destination or action target.
- If the composer has text and the target is `TicketIntake`, `Enter` launches a new pre-ticket Intake from the typed text; the selected Ticket is not used. Existing-Ticket Intake for a selected `Clarify` action is currently reached only from the blank-composer selected-row action path.
- Therefore, “composer has text while a Ticket row is selected” currently has no actionable selected-Ticket semantics. The selected row may still be visible/navigation state, but it should not be displayed as if the typed text will act on that Ticket.

Additional requirement:

- The Panel display should avoid suggesting that typed composer text is scoped to or sent to the selected Ticket unless the implementation explicitly adds such a feature.
- If text composer mode is active, selected-row Ticket action hints should be de-emphasized, hidden, or explicitly separated from the composer destination/action.
- If the future design wants typed text to refine a selected Ticket, that must be an explicit product decision and not implied by the current mixed UI.

---

<!-- event: comment author: intake at: 2026-06-09T09:35:47Z -->

## Comment

## UX exploration note: contextual input instead of always-visible ambiguous composer

User raised a broader display/operation concern:

- The always-visible bottom composer may be intrinsically ambiguous in the Panel because it is not obvious what the box is for at any moment.
- The current UI mixes selected-list-item actions with global text submission. Even if labels are improved, a fixed bottom box can still look like it applies to the selected Ticket/Pod when it may actually send to Companion or launch new Intake.
- A potentially clearer direction is to show an input box contextually for the selected list item/action, rather than keeping one always-visible global box whose meaning changes.

Potential design direction for implementation consideration:

- Default Panel state emphasizes list navigation and selected-row actions; no ambiguous always-active global input is required.
- Immediate row actions can remain `Enter`-driven when no text is needed.
- Actions that need text open a contextual input box attached to, or clearly associated with, the selected row/action.
- Global text destinations such as workspace Companion or new Ticket Intake should be explicit actions/targets, not visually conflated with the selected Ticket action state.
- If a persistent bottom input remains, it should have one unambiguous purpose, such as “Ask Companion”, and selected Ticket action hints should be visually separate from it.

This is not yet a final binding redesign decision. Treat it as reviewer/orchestrator context for the same Ticket: the implementation should prioritize making the input box’s scope and destination obvious, and should avoid relying only on denser status labels to explain a mixed interaction model.

---

<!-- event: plan author: intake at: 2026-06-09T09:46:50Z -->

## Plan

## Refinement: explicit focus model for row actions vs global composer

User clarified a more concrete interaction model to reduce the ambiguity between selected list actions and global text submission.

Desired direction:

- The Panel should distinguish at least two visible interaction states:
  - row/list focus: the user is hovering/selecting a Ticket/Pod/list item and can inspect or choose actions for that item;
  - global composer focus/no selection: the bottom composer is used to send text to a global target such as Companion or new Ticket Intake.
- `Esc` should be able to clear the current row/item selection or action focus and return to the global composer/no-selection state.
- When selection is cleared, the bottom composer can be used for Companion or new Intake target submission without implying that text applies to a selected Ticket.
- For a selected Ticket/list item, Left/Right should navigate into or out of the item’s available actions.
- Pressing Right while hovering a Ticket should expand or focus the available actions for that item.
- The action-focused display should make available actions selectable, such as text-related actions, `Queue`, or other valid item actions.
- Up/Down should continue to navigate list items in list focus; when action focus is active, implementation may define whether Up/Down changes focused action or returns to item navigation, but the resulting behavior must be visible and predictable.
- Text submission for a selected item should be explicit through an item/action-focused UI. Typed text in the global bottom composer should not silently apply to a selected Ticket.

Implications / reviewer focus:

- Avoid a mixed state where a Ticket is visually selected, a bottom composer has text, and the UI does not clearly say whether Enter affects the selected Ticket or the global target.
- The display should make focus explicit: selected item, selected item action, or global composer target.
- `Esc` behavior must be specified carefully because current Panel uses `Esc` as quit. A reasonable implementation may make `Esc` clear action/list focus first and reserve quit for `Ctrl+C`/`Ctrl+D`, or quit only when already in no-selection/global state, but the chosen behavior must be clear and tested.
- `Tab` remains the target-switching key for global composer targets, with completion priority if completion is active.
- Bare letter shortcuts must not be reintroduced.

This refinement supersedes the earlier weaker idea of only improving labels around an always-visible ambiguous composer. The preferred UX direction is an explicit focus/action model: list hover -> Right opens item actions -> choose action; Esc backs out to global composer/no-selection; global composer sends only to explicit global target.

---

<!-- event: decision author: intake at: 2026-06-09T09:50:15Z -->

## Decision

## Decision: Panel quit key and Esc focus behavior

User decided that Panel quit should be available through `Ctrl+C`.

Binding decision update:

- `Ctrl+C` is the Panel quit key.
- `Esc` should not be the primary quit action in the redesigned Panel interaction model.
- `Esc` should be available for backing out of UI focus states, such as item action focus or selected-row focus, toward the no-selection / global composer state.
- If the implementation preserves any `Esc` quit behavior for compatibility, it must not conflict with the new focus-clear/back-out behavior and must be clearly justified in the implementation report.

Reviewer focus:

- Verify that `Esc` does not unexpectedly exit the Panel when the user is trying to leave item/action focus.
- Verify that `Ctrl+C` exits reliably from the redesigned Panel states.

---

<!-- event: state_changed author: workspace-panel at: 2026-06-09T10:01:08Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-06-09T10:03:41Z -->

## Decision

Routing decision: waiting_capacity_parallelism

This Ticket remains queued for now.

Reason:
- `20260609-093050-001` is being accepted as the active core tool execution API migration.
- This Ticket is TUI panel/composer UX work and is implementation-ready, but it is better sequenced after the core tool API migration to keep review/validation focused and avoid broad parallel changes.

State decision:
- Leave this Ticket `queued`.
- Do not create a worktree or spawn a coder yet.

Next action:
- Re-read this Ticket after the ToolExecutionContext API migration lands, or earlier if explicitly prioritized.

---

<!-- event: state_changed author: orchestrator at: 2026-06-09T10:19:34Z from: queued to: inprogress reason: orchestrator_acceptance_parallel_capacity field: state -->

## State changed

Accepted queued implementation under the updated parallel-capacity policy. This Ticket is TUI panel/composer UX work and is independent from the active ToolExecutionContext migration except for normal workspace validation. It can run in a separate worktree with separate write scope.

---

<!-- event: decision author: orchestrator at: 2026-06-09T10:19:34Z -->

## Decision

Routing decision: implementation_ready_parallel

Updated user instruction: prefer parallel work when Tickets are independent or expected conflicts are small/manageable.

Reason:
- This Ticket is focused on TUI panel display/composer key handling.
- It is independent from the active ToolExecutionContext API migration and the TicketList output work, aside from normal shared validation.
- It can run in a separate worktree with a separate Coder scope.

IntentPacket:

Intent:
- Improve Panel display and composer key handling so global composer text entry, selected-row actions, target switching, and focus states are explicit and less ambiguous.

Binding decisions / invariants:
- Remove `Ctrl+T` from Panel top line/actionbar/help and stop using it for Panel target switching.
- Use `Tab` for Panel target switching, but completion state has priority when active.
- Preserve no bare-letter shortcuts; normal typed letters go to composer text.
- Share or abstract composer editing/key handling rather than adding ad-hoc Panel-only editing behavior.
- Support normal composer editing operations including cursor movement, line start/end, deletion, history, and `Ctrl+Left` / `Ctrl+Right` where terminal events make that possible.
- Make focus/model explicit: selected row, item action focus, or global composer target.
- Global composer text must not imply it applies to a selected Ticket unless such a feature is explicitly added.
- `Esc` backs out of action/list focus toward global composer/no-selection; `Ctrl+C` is the reliable quit key.

Reviewer focus:
- Verify the UI answers “what will Enter do now?” in selected-row/action/global-composer states.
- Verify target switching/completion `Tab` priority.
- Verify composer key handling is shared/compatible with normal TUI where practical.
- Verify no bare-letter shortcuts are reintroduced.

Validation:
- Focused TUI/panel key handling/display tests where feasible.
- Manual or test evidence for top/actionbar `Ctrl+T` removal and `Tab` target switch.
- `cargo fmt --check`, `git diff --check`, `cargo run -q -p yoi -- ticket doctor`, `cargo check --workspace`, `nix build .#yoi`.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-09T10:21:06Z -->

## Implementation report

Implementation routing started in parallel with the active ToolExecutionContext and TicketList work.

Worktree/branch:
- Worktree: `.worktree/panel-display-composer-keys`
- Branch: `panel-display-composer-keys`
- Base/routing commit: `d95b3ff ticket: route parallel ticketlist and panel work`

Spawned sibling Coder Pod:
- `coder-panel-display-composer-keys`
- Scope: non-recursive read on parent workspace root plus recursive write limited to the child worktree.

Parallel-routing rationale:
- User asked to prefer parallel work when conflicts are absent or expected small.
- This Ticket is primarily TUI panel/composer UX and is independent from the core ToolExecutionContext migration and TicketList output shaping.
- Coder was instructed to avoid Ticket backend/tool output changes unless necessary.

---

<!-- event: implementation_report author: coder at: 2026-06-09T10:49:47Z -->

## Implementation report

Implemented panel display/composer key handling updates on branch `panel-display-composer-keys`.

Summary:
- Added shared `composer_keys` keymap for readline-style composer editing and reused it from both normal TUI input handling and workspace panel input handling.
- Removed Panel `Ctrl+T` target switching; Panel now uses `Tab` for global composer target switching. `Ctrl+T` is no longer a Panel target shortcut.
- Kept bare letters as composer text and added focused tests for `j/k/o/r` text entry.
- Added explicit Panel focus state (`global composer`, `selected row`, `item action`) and status/actionbar/title wording that distinguishes global composer target from selected rows.
- `Esc` clears Panel selection/focus back to the global composer target; `Ctrl+C` remains quit.
- Added shared key handling coverage for word motion/delete and Panel focus/target transitions.

Files changed:
- `crates/tui/src/composer_keys.rs`
- `crates/tui/src/lib.rs`
- `crates/tui/src/multi_pod.rs`
- `crates/tui/src/single_pod.rs`
- `crates/tui/src/workspace_panel.rs`

Validation:
- `cargo test -p tui composer_keys` — passed
- `cargo test -p tui multi_composer` — passed
- `cargo test -p tui multi_ctrl_t_does_not_switch_composer_target` — passed
- `cargo test -p tui multi_bare_panel_letters_append_to_composer_and_arrows_select_when_blank` — passed
- `cargo test -p tui multi_esc_clears_panel_focus_without_quitting` — passed
- `cargo fmt --check` — passed
- `git diff --check` — passed
- `cargo run -q -p yoi -- ticket doctor` — passed (`doctor: ok`)
- `cargo check --workspace` — passed
- `nix build .#yoi` — passed

Commit: `20f06b3` (`tui: clarify panel focus and composer keys`).

Reviewer notes:
- The workspace panel does not currently expose an active completion popup/source; the Tab handling path is now isolated as the target-switch path, with completion ownership documented in the shared key handling boundary for future completion integration.
- Row Enter behavior is preserved for dispatch/open, while Right marks item-action focus and status text makes that model explicit.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-09T10:55:20Z -->

## Implementation report

Coder implementation completed and is ready for sibling review.

Coder Pod:
- `coder-panel-display-composer-keys`
- Commit: `20f06b3 tui: clarify panel focus and composer keys`
- Worktree status before review: clean branch `panel-display-composer-keys`
- Stopped after collecting output to reclaim delegated worktree scope.

UI/key handling design:
- Panel target switching moved from `Ctrl+T` to `Tab`.
- `Ctrl+T` no longer switches targets in Panel.
- Bare letters such as `j/k/o/r` remain composer text, not shortcuts.
- Panel focus is explicit: global composer, selected row, or item action.
- Composer send target is displayed as global composer target and does not imply selected Ticket target.
- `Esc` clears Panel selection/focus toward global composer/no-selection; `Ctrl+C` remains quit.
- `Right` can move into item action focus while row `Enter` behavior is preserved.

Implementation summary:
- Added shared `composer_keys` keymap used by normal TUI and workspace panel.
- Shared editing covers insert, Alt+Enter newline, backspace/delete, left/right, word left/right, word delete, Home/End; caller keeps Up/Down history/list/completion context.
- Updated Panel help/top/actionbar wording to remove `Ctrl+T` and ambiguous empty Enter wording.
- Added/updated Panel tests for Tab switching, Ctrl+T non-switching, bare-letter regression, shared word motion/delete, Esc focus clear, and Ctrl+C quit.

Changed files:
- `crates/tui/src/composer_keys.rs`
- `crates/tui/src/lib.rs`
- `crates/tui/src/multi_pod.rs`
- `crates/tui/src/single_pod.rs`
- `crates/tui/src/workspace_panel.rs`

Coder validation reported passed:
- `cargo test -p tui composer_keys`
- `cargo test -p tui multi_composer`
- `cargo test -p tui multi_ctrl_t_does_not_switch_composer_target`
- `cargo test -p tui multi_bare_panel_letters_append_to_composer_and_arrows_select_when_blank`
- `cargo test -p tui multi_esc_clears_panel_focus_without_quitting`
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `cargo check --workspace`
- `nix build .#yoi`

Residual note:
- Panel currently has no active completion popup/source, so completion-priority for `Tab` is expressed as the caller consuming Tab before target switching when completion exists.

---
