<!-- event: create author: "yoi ticket" at: 2026-07-02T13:59:17Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: plan author: hare at: 2026-07-02T14:00:20Z -->

## Plan

Workspace Browser Settings shell の先行 Ticket として詳細化した。

方針:
- Settings は admin/user/permission 管理ではなく、現在の local Workspace Backend process と workspace-local config を扱う surface とする。
- Runtime Connections / Backend Config / Workspace Identity の section 受け皿を先に作る。
- この Ticket では mutation や Runtime connection 永続化は実装しない。
- 後続 Runtime connection 管理 Ticket `00001KWHHRTM9` はこの Settings shell を前提にする。


---

<!-- event: decision author: hare at: 2026-07-02T14:09:54Z -->

## Decision

Settings shell Ticket の位置づけを修正した。

Decision:
- `Workspace-local Settings` という独自呼称はやめ、普通に Settings/Admin surface として扱う。
- 管理画面ではない、という意味ではない。
- 現時点で user / permission / multi-user authorization が無いため、それらがあるかのような fake UI / 文言を作らない、という制約だけを明記する。
- 後続 Runtime connection management Ticket の依存先表記も Settings/Admin shell に修正した。


---

<!-- event: intake_summary author: hare at: 2026-07-02T14:18:50Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-02T14:18:50Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-07-02T14:24:54Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-07-02T14:25:39Z -->

## Decision

Routing decision:

Workspace Dashboard Queue authorized Orchestrator routing. Current state was inspected before implementation side effects.

Findings:
- Ticket state: `queued`
- Blocker relations: none
- Current `inprogress` Tickets: 0
- Orchestration worktree: clean

Decision:
- Accept this Ticket for implementation and transition `queued -> inprogress` before child worktree / Pod side effects.

Plan:
- Worktree: `/home/hare/Projects/yoi/.worktree/00001KWHJ0XH6-settings-admin-shell`
- Branch: `work/00001KWHJ0XH6-settings-admin-shell`
- Coder Worker will implement Workspace Browser Settings/Admin shell and navigation with focused `web/workspace/**` write scope.
- Reviewer Worker will verify route rendering, navigation, no fake user/role/permission model, placeholder sections, sanitized diagnostic/restart-required patterns, non-leak of raw path/secret/token/socket/runtime store path, and existing Worker Console/Sidebar continuity.

Non-goals preserved:
- no Runtime connection add/delete/test implementation
- no settings mutation API
- no backend config editor or secret store UI
- no user/permission UI
- no Runtime live register/unregister
- no Nix/package changes expected

Operational note:
- User instructed not to use `StopPod` for now after repeated stop/hang symptoms. Cleanup will skip role-Pod shutdown and remove only child worktree/branch unless explicitly reauthorized.

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-07-02T14:25:48Z from: queued to: inprogress reason: authorized_unblocked_queue_acceptance field: state -->

## State changed

Dashboard Queue authorized routing. The Ticket has no blocker relations, no other Ticket is inprogress, and the orchestration worktree is clean. Accepting implementation before child worktree / role Pod side effects.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-07-02T14:26:39Z -->

## Implementation report

Implementation routing diagnostic:

Attempted to spawn Coder Worker `yoi-coder-00001KWHJ0XH6-settings`, but `SpawnPod` failed before child startup with:

```text
failed to spawn `/home/hare/.cargo/bin/yoi (deleted) pod`: No such file or directory (os error 2)
```

This appears to be a runtime executable availability issue in the current Orchestrator process, not a Ticket/code implementation result.

Operational response:
- The Ticket is already `inprogress` and the child worktree/branch exists.
- Because role Pod spawning is temporarily unavailable and this Ticket is web-only / narrow-scope, Orchestrator will implement the small Settings/Admin shell directly in the child worktree, then route review if spawning becomes available; otherwise Orchestrator will perform focused validation and record the limitation.
- `StopPod` remains unused per user instruction.

---
