<!-- event: create author: "yoi ticket" at: 2026-07-10T21:16:22Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: decision author: hare at: 2026-07-10T21:23:27Z -->

## Decision

Worker 側が持つ workspace 情報は `Option<WorkspaceId>` に留める。`WorkspaceBackendRef`、endpoint、auth/SecretRef、adapter/client の materialization は Runtime/host 側の責務とし、Worker は Runtime から注入された scoped `WorkspaceClient` / handle 経由で workspace-aware API に request する。


---

<!-- event: plan author: hare at: 2026-07-10T21:24:57Z -->

## Plan

整理結果:

1. 00001KX6Y2A9Q WorkerFilesystemAuthority
   - hard dependency なし。
   - no-workdir を filesystem authority none として型で表現する基盤。

2. 00001KX6WVNPD Embedded no-workdir Worker authority policy
   - depends_on: 00001KX6Y2A9Q。
   - UI/API の workdir 任意化と embedded no-workdir tool surface 制御を実装する。

3. 00001KX6Y6ZEA WorkspaceBackend / workspace_id 分離
   - depends_on: 00001KX6Y2A9Q。
   - workspace_root path 依存を workspace_id + Runtime-injected WorkspaceClient へ移行する。
   - embedded no-workdir MVP をブロックしない。

Hard dependencies は WorkerFilesystemAuthority を共通前提に限定する。WorkspaceBackend 分離は embedded no-workdir policy の MVP には related だが depends_on にはしない。


---

<!-- event: decision author: hare at: 2026-07-10T22:00:16Z -->

## Decision

Clarification: this ticket should remove Worker-level `workspace_root: PathBuf`, not leave it as a staged authority surface. Local paths may remain only inside Runtime/host backend adapters or `WorkerFilesystemAuthority::Local`; Worker workspace identity is `Option<WorkspaceId>` and workspace operations go through an injected `WorkspaceClient` / handle.


---

<!-- event: intake_summary author: hare at: 2026-07-10T22:01:46Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-10T22:01:46Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-07-10T22:03:27Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-07-10T22:03:57Z -->

## Decision

Routing decision: blocked_by_dependency

Reason:
- Dashboard queue authorization was inspected, but this Ticket has an explicit typed `depends_on` relation to `00001KX6Y2A9Q`。
- `00001KX6Y2A9Q` is currently `inprogress` and is refactoring Worker filesystem authority / cwd removal / no-workdir boundaries。
- This Ticket replaces Worker `workspace_root` path usage with `WorkspaceBackend`, which builds on the same Worker struct/context/constructor boundary。
- Starting now would likely conflict with the active filesystem-authority refactor and obscure review boundaries。
- Therefore this routing pass leaves the Ticket `queued` and does not record `queued -> inprogress`, create a worktree, or spawn role Pods。

Evidence checked:
- Ticket body / recent thread / artifacts。
- `TicketRelationQuery(00001KX6Y6ZEA)`: includes dependency on `00001KX6Y2A9Q`。
- `TicketList(inprogress)`: `00001KX6Y2A9Q` is the active inprogress Ticket。
- Orchestrator worktree status: clean。

Next action:
- Re-route this Ticket after `00001KX6Y2A9Q` is approved, merged, validated, and closed。
- Escalate only if a human explicitly requests combined implementation in the same branch/worktree。

---

<!-- event: decision author: orchestrator at: 2026-07-10T23:38:54Z -->

## Decision

Routing decision: hold_queued_due_migration_boundary_conflict

Reason:
- 前提 `00001KX6Y2A9Q` は closed になったため dependency 自体は解消済み。
- ただし、この routing pass では `00001KX6WVNPD` を先に受理した。
- `00001KX6WVNPD` は embedded no-workdir policy MVP であり、WorkspaceBackend migration を必要としない小さい境界変更である。
- この Ticket は Worker `workspace_root` path を `WorkspaceBackend` に置き換える refactor で、Worker struct/context/constructor/runtime spawn/UI API 周辺に触れる。
- `00001KX6WVNPD` も同じ Worker create / embedded runtime / no-workdir authority surface に触るため、並行 branch で進めると conflict と review boundary 混濁の可能性が高い。
- よって、この Ticket は `queued` のまま保持し、`00001KX6WVNPD` が reviewed/merged/closed になってから再 routing する。

Evidence checked:
- Ticket body / thread / relations / orchestration plan。
- `TicketRelationQuery(00001KX6Y6ZEA)`: prerequisite `00001KX6Y2A9Q` は closed。
- `TicketList(inprogress)`: routing pass 中に `00001KX6WVNPD` を accepted for implementation。
- Orchestrator worktree status: clean。

Next action:
- `00001KX6WVNPD` の implementation/review/merge/close を完了後、この Ticket を再 routing する。
- 人間が combined branch を明示希望する場合のみ、この待機判断を再検討する。

---
