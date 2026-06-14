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
