---
title: 'Profile launch should preserve override scope allowances'
state: 'inprogress'
created_at: '2026-06-20T10:48:57Z'
updated_at: '2026-06-20T11:54:59Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-20T11:52:33Z'
---

## 背景

`yoi pod` の Profile launch で workspace-local `.yoi/override.local.toml` に `[[scope.allow]]` を追加しても、起動後の Pod が追加 scope を読めない問題がある。

調査では、resolver は override を検出しており、Pod metadata の `resolved_manifest_snapshot.profile.workspace_override` に override path も記録されていた。一方で、最終的な `resolved_manifest_snapshot.scope.allow` には workspace root の write scope だけが残り、override 由来の追加 read scope が消えていた。

原因は `crates/pod/src/entrypoint.rs` の `apply_profile_launch_policy()` が Profile launch 時に `manifest.scope` を `workspace_scope(...)` で丸ごと再代入しているため。`crates/manifest/src/profile.rs` で workspace override は一旦 merge されるが、その後段で scope が上書きされる。

再現例:

```toml
# /home/hare/Projects/yoi-discord-bridge/.yoi/override.local.toml
[[scope.allow]]
target = "/home/hare/Projects/yoi"
permission = "read"
recursive = true
```

`yoi-discord-bridge` Pod の metadata では `workspace_override` は上記 override を指すが、`resolved_manifest_snapshot.scope.allow` に `/home/hare/Projects/yoi` が含まれない。

## 要件

- Profile launch policy は workspace 用の安全な既定 scope / delegation を付与しつつ、Profile/override で明示された追加 `scope.allow` を失わない。
- workspace root write scope と `.worktree` write deny の既定挙動は維持する。
- Ticket role 用の Profile launch policy でも、既存の role 制約を破らない形で追加 scope の扱いを明確化する。
- `resolved_manifest_snapshot` に保存される最終 Manifest が、実際に model/tool に提示される readable/writable scope と一致する。

## 受け入れ条件

- `.yoi/override.local.toml` の追加 `[[scope.allow]]` が Profile launch 後の `resolved_manifest_snapshot.scope.allow` に残ることをテストで確認する。
- 通常 Pod launch で workspace root write scope と `.worktree` write deny が引き続き付与されることを確認する。
- Ticket role launch の scope/delegation 既定が壊れていないことを確認する。
- 既存 metadata snapshot を restore する場合に override が再評価されない挙動は、今回の修正対象外または明確に別問題として扱う。
