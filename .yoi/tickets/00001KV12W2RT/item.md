---
title: 'Panel Ticket rows を2行表示にして gate 情報を分離する'
state: 'done'
created_at: '2026-06-13T18:10:57Z'
updated_at: '2026-06-14T06:42:04Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['tui', 'workspace-panel', 'ticket-relations', 'mouse-input', 'layout']
queued_by: 'workspace-panel'
queued_at: '2026-06-14T06:08:41Z'
---

## Background

Workspace Panel の Ticket row は現在 1 行に `state / title / status / action` を圧縮している。これにより、canonical state と relation / queue blocker から導出される execution gate が混ざりやすい。

例: `state: ready` かつ unresolved `depends_on` がある Ticket は、正本としては ready のままでよい。しかし Panel では queue 可能ではなく、`Gate: waiting` として見せる必要がある。現在の実装では relation blocker が `PanelRowKind::Blocked` / red / `Edit` に寄り、正常な依存待ちが error や人間入力要求のように見える。

Panel の Ticket 表示を default 2 行にして、1 行目に正本 state/title、2 行目に Ticket id と derived gate/action/reason を表示する。選択状態は三角 `▶` ではなく、複数行をまとめる左罫線 `|` で示す。

## Target layout

Default Ticket row is two visual lines:

```text
  ready  Extend pod::feature API for external protocol-backed capability providers
  00001KTR81P9X | Gate: waiting · depends_on 00001KV0SP0TY
```

Selected Ticket row uses a left vertical marker instead of a triangle so the two lines read as one selected item:

```text
| ready  Extend pod::feature API for external protocol-backed capability providers
| 00001KTR81P9X | Gate: waiting · depends_on 00001KV0SP0TY
```

Queueable ready Ticket example:

```text
  ready  Remove feature-layer HostAuthority model
  00001KV0SP0TY | Gate: queueable · Action: Queue
```

Planning Ticket example:

```text
  planning  Some unclear work item
  00001XXXXXXX | Gate: needs planning · Action: Clarify
```

## Requirements

- Ticket rows in Workspace Panel render as two visual lines by default.
  - Line 1: canonical workflow state + title.
  - Line 2: ticket id + derived gate/action/reason summary.
- Preserve the Ticket schema model.
  - Do not add a persisted `waiting` state.
  - Continue treating `state` as canonical lifecycle state.
  - Derive gate/waiting/queueable from relations and current Panel context.
- Relation blockers should be displayed as execution gate information, not as canonical state replacement.
  - `state: ready` with unresolved `depends_on` shows `ready` on line 1.
  - Line 2 shows `Gate: waiting · depends_on <id>`.
  - Queue action is disabled/not offered while blockers are unresolved.
  - Normal dependency wait should not use red/error styling or `Edit` as the primary action.
- Keep truly problematic states visually distinct.
  - Invalid/corrupt/unusable Ticket data or user-decision-required blockers may still use stronger warning styling.
  - Normal relation ordering waits should use a waiting/amber/dim style rather than red.
- Replace selected-row triangle marker with a multi-line grouping marker.
  - Selected Ticket row uses `|` on each visual line.
  - Non-selected Ticket row uses leading spaces aligned with the selected marker.
  - Pod rows may keep existing one-line marker behavior unless changing them is necessary for layout consistency.
- Update list selection and scrolling to account for multi-line Ticket rows.
  - Selection remains one logical Ticket per two visual lines.
  - Keyboard up/down moves by logical row, not visual line.
  - Mouse click on either visual line selects the same Ticket and does not dispatch the action.
  - Mouse wheel behavior remains intact.
- Keep row layout robust for narrow terminals.
  - Truncate title and gate summary with ellipsis.
  - Avoid brittle fixed-width column alignment beyond small state/id labels.
- Update target/action status line wording as needed.
  - Selected ready/waiting Ticket should communicate `queue disabled` or `waiting for <blocker>` instead of simply `ready · Enter Edit`.

## Acceptance criteria

- Workspace Panel unit tests cover two-line Ticket row rendering for at least queueable ready, ready+waiting, planning, queued/inprogress, and done/closed cases where practical.
- A ready Ticket with unresolved `depends_on` renders line 1 with `ready` and line 2 with `Gate: waiting` plus blocker identity.
- The same ready+waiting Ticket does not have `NextUserAction::Queue` and does not use `NextUserAction::Edit` merely because of a normal dependency wait.
- The ready+waiting Ticket is not rendered with the red/UserReply priority used for human-error/user-reply states.
- Selected Ticket rows use `|` on all visual lines belonging to that Ticket; no `▶` triangle is used for those multi-line rows.
- Mouse click on either visual line selects the same logical Ticket without dispatching its action.
- Existing mouse wheel behavior continues to scroll/select as before.
- Existing Panel E2E expectations are updated or extended if they depend on one-line rows or triangle markers.
- Validation: focused `tui` tests, affected E2E if practical, `cargo build -p yoi`, and `nix build .#yoi`.

## Non-goals

- Adding a persisted `waiting` Ticket state.
- Redesigning the whole Panel into full cards or a detail pane.
- Changing Ticket lifecycle transition rules.
- Reworking Pod row layout unless required for shared rendering primitives.

## Related work

- Ready Ticket blocked by dependency example: `00001KTR81P9X` depends on `00001KV0SP0TY`.
- Ticket relation blockers are already surfaced by `TicketShow` and `WorkspacePanelViewModel`.
- Mouse selection / wheel behavior follow-up E2E: `00001KV10SN02`.
