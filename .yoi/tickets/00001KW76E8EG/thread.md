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

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-28T18:40:58Z -->

## Implementation report

Implementation kickoff:

- Worktree created: `/home/hare/Projects/yoi/.worktree/00001KW76E8EG-embedded-runtime-fs-store`
- Branch: `work/00001KW76E8EG-embedded-runtime-fs-store`
- Coder Worker spawned: `yoi-coder-00001KW76E8EG-fs-store`

Scope notes:
- Coder write scope is limited to `crates/workspace-server/**`, `crates/worker-runtime/**`, `crates/worker/**`, root `Cargo.toml`, `Cargo.lock`, and `package.nix` if needed.
- Coder is instructed not to modify Ticket records and not to operate in root/orchestration worktrees.

Implementation focus:
- enable `worker-runtime` `fs-store` feature in `workspace-server`
- add `ServerConfig` embedded Runtime store root
- default store root under user data / workspace id, not under project records or `.yoi/tickets`
- add fs-store backed `EmbeddedWorkerRuntime` constructor using `Runtime::with_fs_store_and_execution_backend`
- normal WorkspaceApi constructor uses fs-store, not `new_memory_with_execution_backend`
- restart restores Runtime catalog / Worker snapshot / transcript / ConfigBundle store
- live execution handle is not falsely connected after restart; stale/unconnected diagnostic/projection remains explicit
- Browser-facing APIs do not expose store root / raw path / execution handle
- tests cover fs-store restore, ConfigBundle/transcript restore, execution stale/unconnected restore, memory-backed production bypass absence, and store-root non-leak

---

<!-- event: implementation_report author: yoi-coder-00001KW76E8EG-fs-store at: 2026-06-28T19:05:47Z -->

## Implementation report

Implementation report:

実装・検証・commit 完了。Ticket ファイル編集、merge、close、cleanup は未実施。

Commit:
- `736b05c6 feat: persist embedded workspace runtime in fs store`

変更概要:
- `workspace-server` で `worker-runtime` の `fs-store` feature を有効化。
- `ServerConfig` に `embedded_runtime_store_root` を追加し、既定値を user data 配下の `workspace-server/<workspace_id>/embedded-runtime` に設定。
- 通常の `WorkspaceApi` 生成経路を `EmbeddedWorkerRuntime::new_memory_with_execution_backend` から `new_fs_store_with_execution_backend` に切り替え。
- `Runtime::with_fs_store_and_execution_backend` を使う fs-store backed constructor を追加。
- 再起動後の Worker は live handle 接続済みと偽らず、`stale` / `unconnected` の診断を Browser API projection に残すようにした。
- store root / raw path / handle が Browser API に出ないことをテスト。
- `package.nix` の `cargoHash` を更新。

追加・更新テスト:
- fs-store restart restore:
  - catalog / Worker snapshot
  - ConfigBundle
  - transcript
  - stale execution projection / input rejection
- store root isolation / non-leak
- 既存 workspace-server tests は明示的 temp store root を使うよう調整。

Validation:
- `cargo fmt`: success
- `cargo fmt -- --check`: success
- `cargo test -p yoi-workspace-server`: success
- `cargo test -p worker-runtime --features "ws-server fs-store"`: success
- `cargo check -p yoi`: success
- `git diff --check`: success
- `nix build .#yoi`: success

作業後状態:
- branch: `work/00001KW76E8EG-embedded-runtime-fs-store`
- `git status --short`: clean

Residual risks:
- 特記すべき未解決リスクなし。再起動後の実行 handle は復元せず stale/unconnected として扱う設計どおり。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-28T19:06:09Z -->

## Implementation report

Review kickoff:

Reviewer Worker spawned for `00001KW76E8EG` implementation.

- Reviewer Worker: `yoi-reviewer-00001KW76E8EG-fs-store`
- Review target commit: `736b05c6 feat: persist embedded workspace runtime in fs store`
- Review mode: read-only implementation worktree review

Review focus:
- `workspace-server` enables `worker-runtime` `fs-store` feature
- `ServerConfig` has authoritative embedded Runtime store root defaulting under user data / workspace id, not project records
- normal `WorkspaceApi` uses fs-store backed `EmbeddedWorkerRuntime`, not memory-backed runtime
- restart restores catalog / Worker snapshot / ConfigBundle / transcript
- execution handle after restart is stale/unconnected diagnostic, not falsely connected
- Browser APIs do not leak store root / raw path / execution handle
- production path cannot accidentally use memory-backed runtime
- focused tests cover these behaviors

---

<!-- event: review author: yoi-reviewer-00001KW76E8EG-fs-store at: 2026-06-28T19:15:12Z status: approve -->

## Review: approve

approve

Ticket `00001KW76E8EG` の意図・受け入れ条件に対して、実装は妥当です。ブロッカーは見つかりませんでした。

## 確認した主な根拠

- `workspace-server` が `worker-runtime` の `fs-store` feature を有効化していることを確認。
  - `crates/workspace-server/Cargo.toml`
- `ServerConfig` に `embedded_runtime_store_root` が追加され、通常の `local_dev` 設定では project records / `.yoi/tickets` ではなく、ユーザーデータ配下の workspace id ベースの領域を既定 root にしていることを確認。
  - `crates/workspace-server/src/server.rs`
- 通常の `WorkspaceApi::new` / `new_with_execution_backend` 経路が `EmbeddedWorkerRuntime::new_fs_store_with_execution_backend(...)` を使い、memory store ではなく fs-store backed runtime を構築していることを確認。
  - `crates/workspace-server/src/server.rs`
  - `crates/workspace-server/src/hosts.rs`
- `EmbeddedWorkerRuntime` 側で `EmbeddedRuntimeStore::fs_store(store_root)` + `WorkerRuntime::with_options(..., RuntimeOptions::ws_server())` を使う経路が追加されていることを確認。
- restart 後の復元について、テストで以下が確認されていることを確認。
  - catalog / worker snapshot が復元される
  - `ConfigBundle` が復元される
  - transcript が復元される
  - restart 後の execution handle は接続済み扱いにならず、stale/unconnected diagnostic となる
  - restart 後に input を送っても接続済み実行 handle として扱われない
- Browser/API projection について、serialized response に store root/raw path/handle などが混入しないことを確認するテストが追加されていることを確認。
- production 経路については、通常 `WorkspaceApi` の生成経路が fs-store を使っており、memory-backed runtime に戻る変更は見当たりませんでした。memory constructor は低レベル/テスト用途として残っていますが、通常 server path からは使われていません。

## 実行した検証

read-only 方針に合わせ、worktree 内のソース/Ticket は変更せず、`CARGO_TARGET_DIR` を `/run/user/.../target-review` に逃がして検証しました。

- `git diff --check HEAD^ HEAD`: success
- `cargo fmt --all -- --check`: success
- `CARGO_TARGET_DIR=/run/user/1000/yoi/yoi-reviewer-00001KW76E8EG-fs-store/target-review cargo test -p yoi-workspace-server`: success
- `CARGO_TARGET_DIR=/run/user/1000/yoi/yoi-reviewer-00001KW76E8EG-fs-store/target-review cargo test -p worker-runtime --features ws-server,fs-store`: success
- `CARGO_TARGET_DIR=/run/user/1000/yoi/yoi-reviewer-00001KW76E8EG-fs-store/target-review cargo check -p yoi`: success

検証後の `git status --short` も clean でした。

## 残リスク

- `nix build .#yoi` は reviewer 側では再実行していません。ただし `Cargo.lock` / `package.nix` の更新と Rust 側の check/test は整合しており、報告されているビルド系コマンドは十分 plausible と判断します。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-28T19:16:09Z -->

## Implementation report

Merge / validation report:

Implementation branch was integrated into the orchestration branch after reviewer approval.

Integrated commit:
- `736b05c6 feat: persist embedded workspace runtime in fs store`

Merge commit:
- `888e7b68 merge: embedded runtime fs-store`

Validation in orchestration worktree:
- `cargo fmt --all --check`: success
- `cargo test -p yoi-workspace-server`: success (`38 passed`)
- `cargo test -p worker-runtime --features "ws-server fs-store"`: success (`26 passed` + rest server tests)
- `cargo check -p yoi`: success
- `git diff --check`: success
- `nix build .#yoi --no-link`: success

Outcome:
- Workspace Backend embedded Runtime now uses fs-store backed `worker-runtime` for normal `WorkspaceApi` construction.
- `ServerConfig` carries `embedded_runtime_store_root`, with default user-data/workspace-id local state path rather than project records / `.yoi/tickets`.
- Restart restores catalog / Worker snapshot / ConfigBundle / transcript.
- Restart does not falsely restore live execution handles as connected; stale/unconnected diagnostic/projection remains explicit.
- Browser-facing APIs do not expose Runtime store root / raw path / execution handle.
- Production path no longer uses `new_memory_with_execution_backend`; memory constructor remains for explicit low-level/test use only.

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-28T19:16:18Z from: inprogress to: done reason: implementation_merged_and_validated field: state -->

## State changed

Reviewer approval後に implementation branch を orchestration branch へ merge し、要求された validation が成功したため done に遷移する。

Validation:
- `cargo fmt --all --check`: success
- `cargo test -p yoi-workspace-server`: success (`38 passed`)
- `cargo test -p worker-runtime --features "ws-server fs-store"`: success
- `cargo check -p yoi`: success
- `git diff --check`: success
- `nix build .#yoi --no-link`: success

---

<!-- event: state_changed author: hare at: 2026-06-28T19:16:31Z from: done to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-06-28T19:16:31Z status: closed -->

## 完了

Workspace Backend embedded Runtime を memory-backed から fs-store backed `worker-runtime` へ切り替え、reviewer approval 後に orchestration branch へ merge した。

実装内容:
- `workspace-server` で `worker-runtime` の `fs-store` feature を有効化。
- `ServerConfig` に `embedded_runtime_store_root` を追加。
- default store root を user data 配下の `workspace-server/<workspace_id>/embedded-runtime` に設定し、project records / `.yoi/tickets` と混ぜない設計にした。
- 通常の `WorkspaceApi::new` / `new_with_execution_backend` 経路を fs-store backed `EmbeddedWorkerRuntime::new_fs_store_with_execution_backend(...)` に切り替え。
- `Runtime::with_fs_store_and_execution_backend(...)` を使う constructor を追加。
- Backend restart 後に Runtime catalog / Worker snapshot / transcript / ConfigBundle store が復元されることを focused tests で確認。
- restart 後の live execution handle は connected として偽装せず、stale/unconnected diagnostic/projection として扱う。
- Browser-facing API に Runtime store root / raw path / execution handle を出さないことを確認。
- production path が memory-backed runtime に戻らないよう通常 constructor を fs-store backed にした。

Integrated commit:
- `736b05c6 feat: persist embedded workspace runtime in fs store`
- merge: `888e7b68 merge: embedded runtime fs-store`

Validation:
- `cargo fmt --all --check`: success
- `cargo test -p yoi-workspace-server`: success (`38 passed`)
- `cargo test -p worker-runtime --features "ws-server fs-store"`: success
- `cargo check -p yoi`: success
- `git diff --check`: success
- `nix build .#yoi --no-link`: success

Operational note:
- User instructed not to use `StopPod` for now after repeated stop/hang symptoms. Cleanup will skip role-Pod shutdown and remove only child implementation worktree / branch.

---
