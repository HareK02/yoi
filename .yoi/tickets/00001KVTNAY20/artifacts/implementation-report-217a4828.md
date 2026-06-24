# 実装報告: 00001KVTNAY20

## 変更概要

- `client::spawn` に `PodProcessLaunchConfig` と `PodProcessLaunchOptions` を導入し、低レベルの Pod プロセス起動設定から Ticket role marker を分離した。
- Ticket role 起動は `TicketRoleLaunchPlan::spawn_options()` 経由で hidden CLI marker を渡す形にし、`TicketRoleLaunchResult` に Run 受理証跡 (`TicketRoleLaunchAcceptanceEvidence`) を追加した。
- Workspace server の `LocalRuntimeBridge` を `WorkspaceWorkerRuntime` trait の実装として整理し、hosts/workers 一覧、worker lookup、spawn/stop typed request/result、将来の proxy/stream 接続点を型として追加した。
- Workspace 側の spawn request shape は policy intent ベースにし、browser/API caller から raw `workspace_root` / `cwd` / executable path / raw profile selector を受け取らない形にした。
- Dashboard/TUI 側の直接 spawn 呼び出しを新しい low-level config/options 分離に追従した。

## 変更ファイル

- `crates/client/src/lib.rs`
- `crates/client/src/spawn.rs`
- `crates/client/src/ticket_role.rs`
- `crates/tui/src/dashboard/mod.rs`
- `crates/tui/src/spawn.rs`
- `crates/workspace-server/src/hosts.rs`
- `crates/workspace-server/src/server.rs`

## 検証結果

- `cargo test -p yoi-workspace-server`: 成功
- `cargo check -p yoi`: 成功
- `cd web/workspace && deno task check && deno task build`: 成功
- `cargo test -p client`: 成功（追加確認）
- `git diff --check`: 成功

## コミット

- 実装コミット: `217a4828d73ab553b5406cc7e22e43b1ec7be48e`

## 残リスク / 非ゴールとして残したもの

- `WorkspaceWorkerRuntime::spawn_worker` / `stop_worker` は typed boundary と request/result を用意した段階で、実際の Worker operation UI 完成、stream proxy、remote Host protocol、認可/権限、registry locking までは実装していない。
- low-level launcher は trusted in-process resolver からの追加 CLI args を受け取れるが、Ticket role などのドメイン概念は `PodProcessLaunchConfig` からは除外している。
- TS 型生成やフロントエンド API surface の追加は本 Ticket の非ゴールとして変更していない。
