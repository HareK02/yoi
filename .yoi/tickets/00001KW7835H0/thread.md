<!-- event: create author: "yoi ticket" at: 2026-06-28T13:53:21Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: decision author: hare at: 2026-06-28T14:20:04Z -->

## Decision

調査結果:

- `crates/pod` は現 workspace には存在しない。`Cargo.toml` workspace members / workspace dependencies にも `crates/pod` / `pod = ...` はない。したがって「pod crate を消す」作業は既に過去の移行で遂行済みで、この Ticket では不在確認と再導入防止を扱う。
- 残っている旧 Pod 関連 crate は `crates/pod-registry` と `crates/pod-store`。
- これらが残っている理由は、旧 Pod UI crate ではなく、現 `worker` / `tui` / `yoi` の内部がまだ旧 store / registry を使っているため。

現時点の直接依存:

- `crates/worker/Cargo.toml`
  - `pod-store = { workspace = true }`
  - `pod-registry = { workspace = true }`
- `crates/tui/Cargo.toml`
  - `pod-store = { workspace = true }`
  - `pod-registry = { workspace = true }`
- `crates/yoi/Cargo.toml`
  - `pod-store = { workspace = true }`

主な残存理由:

1. `worker` crate の Spawn/Stop/peer discovery/delegated scope 経路が `pod-registry` を使っている。
   - `worker/src/spawn/tool.rs`: `delegate_scope`, `release_worker`, `default_registry_path`, `LockFileGuard`。
   - `worker/src/spawn/registry.rs`: delegated scope reclaim / release。
   - `worker/src/worker.rs`: top-level install / adopt allocation / segment update / reclaim。
   - `worker/src/discovery.rs`: segment lookup / peer discovery。
   これは旧 child Pod/Worker process orchestration の実行時 authority が残っているため。

2. `worker` crate の metadata/session persistence が `pod-store` を generic Worker store として使い続けている。
   - `worker/src/entrypoint.rs`: `FsWorkerStore`, `CombinedStore`。
   - `worker/src/runtime_adapter.rs`: `FsWorkerStore`, `CombinedStore`, `pod_store_dir`。
   - `worker/src/controller.rs`, `worker/src/worker.rs`, `worker/src/ticket_event_notify.rs`, tests。
   これは名前は Pod だが、`WorkerMetadataStore` / `WorkerMetadata` / `FsWorkerStore` が現 Worker 実行にも必要な metadata/session store として残っているため。削除前に Worker/Runtime store 境界へ move/rename が必要。

3. `tui` crate が旧 local Worker inventory / picker / dashboard で `pod-store` / `pod-registry` を読んでいる。
   - `tui/src/picker.rs`
   - `tui/src/worker_list.rs`
   - `tui/src/dashboard/mod.rs`
   current Runtime API / Backend Worker list へ置換するか、旧 TUI inventory を削除する必要がある。

4. `yoi` CLI の session / worker cleanup が `pod-store` を読んでいる。
   - `yoi/src/session_cli.rs`
   - `yoi/src/worker_cleanup_cli.rs`
   旧 metadata cleanup / referenced session discovery 用。Runtime/worker-runtime store へ置換するか、旧 cleanup CLI を削除する必要がある。

結論:

- `crates/pod` が残っているわけではない。既に消えている。
- `pod-registry` は旧 process/delegation/segment/socket 系 authority として `worker` と TUI に残っている。
- `pod-store` は旧名のまま generic Worker metadata/session store として `worker`、TUI、CLI に残っている。
- 削除作業は「pod crate の削除」ではなく、`pod-registry` の旧 orchestration authority 削除と、`pod-store` の必要部分を Worker/Runtime store に移管して crate を消す作業になる。


---

<!-- event: intake_summary author: hare at: 2026-06-28T14:24:09Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-28T14:24:09Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-28T16:51:00Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-28T16:51:22Z -->

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
- Leave `00001KW7835H0` queued and do not create a child worktree / role Pod yet.
- Reason: `pod-registry` / `pod-store` removal must follow the canonical Runtime Worker creation authority decision and implementation in `00001KW7726H9`. Starting both in parallel would risk deleting/moving store/registry paths while the canonical Worker creation API is still being shaped.

Next routing condition:
- Re-evaluate this Ticket after `00001KW7726H9` is merged, validated, and closed, or after an explicit replan changes the dependency.

---

<!-- event: decision author: yoi-orchestrator at: 2026-06-28T19:17:30Z -->

## Decision

Routing decision:

The previously blocking dependency `00001KW7726H9` is now merged, validated, closed, and cleaned up. Related fs-store persistence Ticket `00001KW76E8EG` is also merged, validated, closed, and cleaned up. Current workspace state was inspected before implementation side effects.

Findings:
- Ticket state: `queued`
- Blocking dependency: `00001KW7726H9` is closed.
- Related persistence Ticket: `00001KW76E8EG` is closed.
- Current `inprogress` Tickets: 0
- Orchestration worktree: clean

Decision:
- Accept this Ticket for implementation and transition `queued -> inprogress` before child worktree / Pod side effects.

Plan:
- Worktree: `/home/hare/Projects/yoi/.worktree/00001KW7835H0-old-pod-crate-cleanup`
- Branch: `work/00001KW7835H0-old-pod-crate-cleanup`
- Coder Worker will remove old `pod-registry` / `pod-store` crates and migrate any remaining authority/persistence responsibilities to Worker/Runtime/session-store boundaries.
- Reviewer Worker will verify crate removal, dependency graph cleanup, Runtime authority invariants, stale compatibility avoidance, Browser/UI/CLI/prompt/test wording, and validation evidence.

Operational note:
- User instructed not to use `StopPod` for now after repeated stop/hang symptoms. Cleanup will skip role-Pod shutdown and remove only child worktree/branch unless explicitly reauthorized.

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-28T19:17:36Z from: queued to: inprogress reason: dependencies_closed_and_queue_continues field: state -->

## State changed

Dependencies are closed, no Ticket is currently inprogress, and the orchestration worktree is clean. Accepting implementation before child worktree / role Pod side effects.

---
