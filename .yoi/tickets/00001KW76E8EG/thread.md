<!-- event: create author: "yoi ticket" at: 2026-06-28T13:24:27Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: plan author: hare at: 2026-06-28T17:01:47Z -->

## Plan

Workspace Backend embedded Runtime の fs-store 永続化について詳細化した。

調査済み事実:
- `worker-runtime` には `Runtime::with_fs_store_and_execution_backend(...)` と `FsRuntimeStoreOptions` が存在する。
- `workspace-server` は現在 `worker-runtime` を `ws-server` feature のみで使っており、`fs-store` feature が有効ではない。
- `WorkspaceApi::new_with_execution_backend(...)` は `EmbeddedWorkerRuntime::new_memory_with_execution_backend(...)` を呼んでいる。
- `ServerConfig` は Runtime store root を持っていない。
- `worker-runtime` の restore は live execution handle を復元せず `WorkerExecutionStatus::unconnected()` / `execution_handle: None` にする。

実装方針:
- `workspace-server` で `worker-runtime` の `fs-store` feature を有効化する。
- `ServerConfig` に embedded Runtime store root を追加する。
- CLI/default path は user data 配下の workspace_id 別 path にする。
- `EmbeddedWorkerRuntime` に fs-store backed constructor を追加し、`Runtime::with_fs_store_and_execution_backend(...)` を通す。
- Workspace Backend の通常 constructor から memory-backed Runtime を使う経路を外す。
- restart 後は catalog / transcript / ConfigBundle を復元し、live execution handle は connected として偽装しない。


---

<!-- event: intake_summary author: hare at: 2026-06-28T17:11:08Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-28T17:11:08Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-28T17:12:16Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-28T17:12:39Z -->

## Decision

Routing decision:

Workspace Dashboard Queue authorized Orchestrator routing, so the Ticket and current workspace state were inspected.

Findings:
- Ticket state: `queued`
- Outgoing dependency: `depends_on 00001KW7726H9`
- Blocking Ticket `00001KW7726H9` is currently `inprogress`.
- Current inprogress Tickets: `00001KW7726H9`
- Orchestration worktree: clean

Decision:
- Leave `00001KW76E8EG` queued and do not create a child worktree / role Pod yet.
- Reason: embedded Runtime fs-store persistence should persist the canonical Runtime Worker creation model, not an embedded-only bypass. That canonical creation path is currently being implemented in `00001KW7726H9`.

Next routing condition:
- Re-evaluate this Ticket after `00001KW7726H9` is merged, validated, and closed, or after an explicit replan changes the dependency.

---

<!-- event: decision author: yoi-orchestrator at: 2026-06-28T18:40:18Z -->

## Decision

Routing decision:

`00001KW7726H9` has been merged, validated, closed, and cleaned up, so this Ticket's dependency is now resolved. Current workspace state was inspected before implementation side effects.

Findings:
- Ticket state: `queued`
- Dependencies on `00001KW55B33B` and `00001KW7726H9` are resolved/closed.
- Current `inprogress` Tickets: 0
- Orchestration worktree: clean
- `00001KW7835H0` remains queued and is related to this fs-store migration. Run embedded Runtime fs-store persistence first, then re-evaluate old Pod crate removal.

Decision:
- Accept this Ticket for implementation and transition `queued -> inprogress` before child worktree / Pod side effects.

Plan:
- Worktree: `/home/hare/Projects/yoi/.worktree/00001KW76E8EG-embedded-runtime-fs-store`
- Branch: `work/00001KW76E8EG-embedded-runtime-fs-store`
- Coder Worker will implement the Workspace Backend embedded Runtime fs-store switch and focused restart/persistence tests.
- Reviewer Worker will verify fs-store feature enablement, ServerConfig store-root authority, restart restore behavior, ConfigBundle/transcript restore, stale execution diagnostic, Browser-facing non-leak, and production memory-backed bypass absence.

Operational note:
- User instructed not to use `StopPod` for now after repeated stop/hang symptoms. Cleanup will skip role-Pod shutdown and remove only child worktree/branch unless explicitly reauthorized.

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-28T18:40:23Z from: queued to: inprogress reason: dependencies_closed_and_queue_continues field: state -->

## State changed

Dependencies `00001KW55B33B` and `00001KW7726H9` are closed, no Ticket is currently inprogress, and the orchestration worktree is clean. Accepting implementation before child worktree / role Pod side effects.

---
