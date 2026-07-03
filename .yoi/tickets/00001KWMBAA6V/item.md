---
title: 'Live-reload Runtime connection registry changes'
state: 'planning'
created_at: '2026-07-03T15:59:48Z'
updated_at: '2026-07-03T15:59:48Z'
assignee: null
---

## 背景

Workspace Settings の Runtime Connections v0 は remote Runtime の追加・削除を `.yoi/workspace-backend.local.toml` に永続化するが、実行中 Backend の `RuntimeRegistry` には反映しない。そのため、追加直後の connection test は通っても、worker launch 候補や実際の runtime routing に使うには Backend restart が必要になり、Settings 上に `runtime_registry_restart_required` warning が出る。

これは安全側の v0 制約として導入したものだが、Settings から接続を追加・テストしてすぐ使う UX では不自然で、接続テストの意味も分かりにくくしている。設定ファイルの desired state と active registry state を分ける方針は維持しつつ、Settings mutation 時に active registry を同期更新できる経路を作る。

## 要件

- remote Runtime add/delete 後、Backend restart なしで active `RuntimeRegistry` に反映される。
- add 成功後、その Runtime が worker 作成候補・worker list/detail/console/event routing など active registry 経由の Browser-facing API から利用可能になる。
- delete 成功後、その Runtime は active registry から除去され、新規 worker launch 候補や routing 対象から外れる。
- persisted config、active registry、connection test / negotiation result は引き続き別概念として扱う。
- Browser-facing API は endpoint/token/config path/socket/session/store path/live handle など authority-bearing internals を漏らさない。
- active worker や console subscription が存在する Runtime の削除時挙動を明示する。v1 では安全に拒否するか、既存 worker/session への影響がないことを保証した上で unregister する。
- registry apply 失敗時の config 永続化との整合性を明示する。config だけ保存されて active registry に反映されない中途半端な状態を、診断なしに成功扱いしない。

## 受け入れ条件

- Settings から remote Runtime を追加した直後に `restart_required` warning なしで利用可能になる。
- 追加直後の Runtime が manual Coding Worker launch の runtime candidate に現れる。
- Settings から remote Runtime を削除した直後に candidate / active registry から消える。
- 接続 test は live registry 反映可否と compatibility/negotiation 結果を区別して表示する。
- active worker が存在する Runtime delete の挙動について test がある。
- backend route tests と必要な web model/UI tests が追加される。
- `cargo test -p yoi-workspace-server` と `cd web/workspace && deno task test && deno task check` が通る。
