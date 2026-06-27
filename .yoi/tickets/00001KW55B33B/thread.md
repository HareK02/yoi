<!-- event: create author: "yoi ticket" at: 2026-06-27T18:26:46Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-06-27T18:58:48Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-27T18:58:48Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-27T19:06:30Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-27T19:08:35Z -->

## Decision

Routing decision: blocked_by_dependency_or_missing_authority

Reason:
- Dashboard Queue 後に Ticket / relations / orchestration plan / workspace state を確認した。
- 本 Ticket は `00001KW55B32Y` (`worker-runtimeにWorker実行Backend境界を追加する`) に `depends_on` relation を持つ。
- `00001KW55B32Y` は本 routing pass で accepted され `inprogress` になった。
- Adapter は execution backend boundary に接続する必要があるため、boundary の shape が review/merge/done になる前に開始しない。

Evidence checked:
- Ticket body: adapter placement/dependency boundary、Profile/config/authority resolution、input/run lifecycle、protocol event bridge、Non-goals。
- Relations: outgoing `depends_on -> 00001KW55B32Y`; incoming dependent `00001KW55B33H`。
- Orchestration plan: blocker record `orch-plan-20260627-190816-1` を追加。
- Workspace state: `00001KW55B32Y` is inprogress; current worktree clean before implementation side effects.

Next action:
- 本 Ticket は queued のまま待機。
- `00001KW55B32Y` が reviewer approve / merge / validation / done になった後に再 routing する。

---

<!-- event: decision author: yoi-orchestrator at: 2026-06-27T20:11:40Z -->

## Decision

Routing decision:

Queue continuation was explicitly requested. The upstream boundary Ticket `00001KW55B32Y` is closed after merge and validation, so this Ticket is now unblocked and accepted for implementation.

Plan:
- Create child implementation worktree:
  - `/home/hare/Projects/yoi/.worktree/00001KW55B33B-worker-runtime-worker-adapter`
- Branch:
  - `work/00001KW55B33B-worker-runtime-worker-adapter`
- Route implementation to a sibling Coder Worker with focused write scope.
- Route read-only review to a sibling Reviewer Worker after implementation.
- Orchestrator retains merge, final validation, Ticket completion, child worktree/branch cleanup, and role Pod shutdown.

Review/implementation focus:
- choose adapter placement without introducing crate dependency cycles, and record the rationale
- connect embedded `worker-runtime` execution backend to existing `worker` crate execution
- preserve Profile/config bundle and authority resolution boundaries
- dispatch input to real Worker run lifecycle, not providerless/fake responses
- bridge `protocol::Event` to Runtime observation bus / Backend WS
- return typed diagnostics/rejections for missing config/provider/busy/unsupported operations
- do not expose raw handles, sockets, credentials, session paths, or manifest paths to Browser-facing APIs
- leave `00001KW55B33H` queued until this adapter is merged/validated

Operational note:
- `StopPod` must be called serially during cleanup; do not use parallel StopPod calls.

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-27T20:11:44Z from: queued to: inprogress reason: dependency_closed_and_queue_continues field: state -->

## State changed

Queue continuation requested by user. Upstream dependency `00001KW55B32Y` is closed with merge and validation complete, and this Ticket has no remaining blockers. Accepting implementation before child worktree / role Pod side effects.

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-27T20:12:31Z -->

## Implementation report

Implementation kickoff:

- Worktree created: `/home/hare/Projects/yoi/.worktree/00001KW55B33B-worker-runtime-worker-adapter`
- Branch: `work/00001KW55B33B-worker-runtime-worker-adapter`
- Coder Worker spawned: `yoi-coder-00001KW55B33B-worker-adapter`

Scope notes:
- Coder write scope is limited to `crates/worker-runtime/**`, `crates/worker/**`, `crates/workspace-server/**`, root `Cargo.toml`, `Cargo.lock`, and `package.nix` if dependency/package changes are needed.
- Coder is instructed not to modify Ticket records and not to operate in root/orchestration worktrees.

Implementation focus:
- adapter placement without crate dependency cycles, with rationale reported back
- connect `worker-runtime` execution backend boundary to existing `worker` crate run lifecycle
- bridge existing `protocol::Event` to Runtime observation bus / Backend WS
- preserve typed diagnostics for missing config/provider/secret and busy/unsupported operations
- avoid fake/providerless responses and Browser-facing raw handle/path/credential/session leaks
- leave downstream Workspace Companion real execution Ticket `00001KW55B33H` queued until this adapter is merged/validated

---

<!-- event: implementation_report author: yoi-coder-00001KW55B33B-worker-adapter at: 2026-06-27T20:51:14Z -->

## Implementation report

Implementation report:

実装完了。マージ / close / cleanup は未実施。

Commit:
- `18526ee36264610048f48b07b5db50ce86852fd2 feat: connect runtime worker execution adapter`

Adapter 配置方針:
- `worker` crate 側に `runtime-adapter` feature 付きで配置。
- 理由:
  - `worker-runtime` は Runtime 境界 / 投影 / Backend trait を持つ下位境界として保ち、実 Worker 実装への依存を持たせない。
  - `worker` crate は既存の `WorkerController` / `WorkerHandle` / `Method::Run` lifecycle を直接扱えるため、fake/canned 応答を挟まずに adapter を組める。
  - `workspace-server` が `worker = { features = ["runtime-adapter"] }` を有効化して embedded Runtime に接続する形にし、crate dependency cycle を避けた。

変更概要:

### `worker` crate

- `crates/worker/src/runtime_adapter.rs` を追加。
- `WorkerRuntimeExecutionBackend` を追加し、`worker_runtime::execution::WorkerExecutionBackend` を実装。
- Runtime worker 作成時に `WorkerController` を spawn し、内部にのみ real `WorkerHandle` を保持。
- Runtime input は `Method::Run { input: vec![Segment::text(...)] }` へ変換して既存 Worker run lifecycle に投入。
- Worker の `protocol::Event` broadcast を購読し、`WorkerExecutionContext::publish_protocol_event` 経由で Runtime observation bus へ橋渡し。
- Provider/profile/config 不足などは spawn/create 時の typed error/rejection として返し、fake assistant text は生成しない。
- v0 double-run policy として queue は持たず、adapter 側 atomic busy flag + Worker status で二重 input を typed busy rejection。
- stop/cancel は既存 `Method::Shutdown` / `Method::Cancel` へ接続。
- Focused test を追加し、fake deterministic LLM client で Runtime input -> real Worker run lifecycle -> assistant output -> `protocol::Event` -> Runtime transcript projection の流れを検証。

### `worker-runtime`

- `observe_worker_event` で受けた real `protocol::Event` から transcript projection を更新。
  - `TextDone` -> Assistant transcript
  - `Error` -> System transcript
  - `ToolResult` -> System transcript
- observation bus には既存 `protocol::Event` をそのまま保持し、subset output model は追加していない。

### `workspace-server`

- embedded Runtime 作成時に `WorkerRuntimeExecutionBackend` を接続。
- `EmbeddedWorkerRuntime::new_memory_with_execution_backend` を追加。
- Browser/API-facing projection に raw `worker::Worker` handle / raw execution handle / socket path / session path / credential / secret ref / manifest path は出していない。

### package/dependency

- `worker` に optional `worker-runtime` dependency + `runtime-adapter` feature を追加。
- `workspace-server` が `worker` の `runtime-adapter` feature を有効化。
- `Cargo.lock` と `package.nix` の `cargoHash` を更新。

Validation:
- `cargo fmt --all`: success
- `cargo fmt --all --check`: success
- `cargo test -p worker --features runtime-adapter runtime_adapter`: success
- `cargo test -p worker-runtime --features ws-server`: success
- `cargo test -p worker-runtime --features "ws-server fs-store"`: success
- `cargo test -p yoi-workspace-server`: success
- `cargo check -p yoi`: success
- `git diff --check`: success
- `nix build .#yoi --no-link`: success

Residual risks / review focus:
- Runtime worker profile 解決は production adapter が通常の Worker profile 解決を使う。workspace-server 起動直後に fake 応答へ fallback せず、profile/provider/config 不足は typed error になる。この挙動が downstream `00001KW55B33H` の Companion UX 設計と合っているか確認が必要。
- Transcript projection は現時点で `TextDone` / `Error` / `ToolResult` を投影。より細かい tool-call lifecycle や thinking block の Browser projection が必要なら downstream で拡張対象。
- Adapter は v0 non-queue policy。二重 input は busy rejection で、background queue/scheduler は実装していない。
- `workspace-server` が `worker` を feature 付きで依存するため、crate layering と public API leak が意図通りかを重点 review してほしい。

---
