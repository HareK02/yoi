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
