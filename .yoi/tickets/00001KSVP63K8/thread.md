<!-- event: create author: tickets.sh at: 2026-05-30T05:37:21Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-05-30T05:38:11Z -->

## Plan

## Initial preflight

Classification: requirements-sync-needed.

The user requirement is clear at the UX level: Enter while running remains an after-run queue, and a separate action should inject supplemental context into the current in-flight run as soon as safe. The exact protocol/history representation is not decided yet and must be designed before implementation.

Critical constraints:
- Do not place injected text into LLM context unless it has first been appended to Worker history / persisted history.
- Do not mutate an active provider stream.
- Consume injected text only at safe boundaries such as before a later LLM request or between tool-call cycles.
- Do not silently drop text; if the active turn cannot accept injection, report/fail closed or explicitly queue.

Design questions to settle before coding:
- TUI action/keybinding/command name.
- Whether existing `Method::Notify` is semantically sufficient or a new typed method is needed.
- Which history item represents user-originated in-flight supplemental context.
- Which Worker/controller boundaries can actually observe injected input before the next LLM call.
- How queued-next-turn vs injected-current-run is displayed.


---
