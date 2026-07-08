---
title: 'Add Workspace settings API and pages for Workspace/Profile editing'
state: 'planning'
created_at: '2026-07-08T08:34:01Z'
updated_at: '2026-07-08T09:25:00Z'
assignee: null
---

## 背景

Workspace Backend は Browser/product Workspace の正本側であり、現状の local backend では Workspace 情報や Profile source を filesystem / project records から読んでいる。一方、Browser には Workspace 情報や Profiles を確認・編集するための明示的な API / 操作ページが十分にない。

Runtime は Workspace filesystem を直接読まない方向にするため、Workspace metadata、Profile registry、Decodal Profile source、override snapshot などの Workspace-derived config は Backend が管理・編集・検証し、Frontend は Backend API 経由で操作する必要がある。

本 Ticket は、Runtime 連携ではなく、Backend <-> Front の Workspace/Profile 管理面を作る実装 slice とする。Decodal Profile source model と `ProfileSourceArchive` の境界は `00001KWZ5KERY` で先に固め、本 Ticket はその後に UI/API 操作面を実装する。

## 実装順序

- depends_on: `00001KWZ5KERY` — Decodal Profile DSL と `ProfileSourceArchive` の source model / validation / import policy を先に実装する。
- then: 本 Ticket — Backend <-> Front の settings API と操作ページを、その source model に合わせて実装する。

## 実装目的

- Browser から Workspace 情報を確認・編集できる。
- Browser から Workspace/Profile source を確認・編集できる。
- Backend API が Workspace/Profile editing の authority になり、Frontend は filesystem path を直接扱わない。
- Profile editing 後に、Backend 側の Profile discovery / validation / launch options に反映される。
- Secret value、Runtime endpoint、host absolute path、runtime-local store path を Browser-facing API に出さない。

## 実装内容

### 1. Workspace settings API を追加する

`/api/w/<workspace-id>/settings` 配下に、Workspace metadata と Profile settings を扱う typed API を追加する。

v0 で扱う Workspace metadata:

- display name。
- workspace id / read-only identity summary。
- repository registry summary / read-only configured repositories。
- Backend local source summary。host absolute path は出さず、safe label / source kind / status だけを出す。

v0 で扱う Profile metadata:

- discovered profile registry entries。
- builtin profile entries。
- workspace/project profile entries。
- selected source kind / profile selector / label / role metadata。
- validation diagnostics。

### 2. Profile source read API を追加する

Frontend が Profiles を編集するため、Backend が safe Profile source model を返す。

実装すること:

- workspace/project `profiles.toml` 相当の registry を structured model として返す。
- Decodal Profile source を safe id で参照して読み出せる。
- imported source は Profile artifact に紐づく bounded artifact として返す。
- Browser-facing response は raw absolute path ではなく `profile_source_id` / `artifact_id` / safe display path を使う。
- artifact size / unsupported source / symlink escape / path escape は typed diagnostic にする。

### 3. Profile source write API を追加する

Browser から Profile registry / Decodal Profile source を更新できる typed mutation API を追加する。

実装すること:

- registry entry の create / update / delete。
- Decodal Profile source の create / update / delete。
- imported source の create / update / delete は v0 で必要なら bounded module root 内だけ許可する。
- write 前に syntax / schema / selector uniqueness / path safety を検証する。
- write 後に Profile discovery を再実行し、diagnostics と effective profile list を返す。
- concurrent update には revision / etag / updated_at 相当の optimistic check を使う。

### 4. Frontend の Workspace settings pages を追加する

既存 SvelteKit workspace-scoped route 配下に settings pages を追加・拡張する。

想定 route:

- `/w/<workspace-id>/settings` Workspace overview settings。
- `/w/<workspace-id>/settings/profiles` Profile list / validation diagnostics。
- `/w/<workspace-id>/settings/profiles/<profile-source-id>` Profile source editor。

UI 要件:

- Workspace display name を編集できる。
- Profile list を source kind / selector / label / diagnostics 付きで見られる。
- Profile registry を編集できる。
- Decodal Profile source は v0 では formatter / syntax highlighter なしの plain text textarea/editor で編集できる。
- 保存前後の validation diagnostics を表示する。
- raw absolute path、Runtime endpoint、secret value を表示しない。
- Browser WorkingDirectory / Worker launch の profile candidates と同じ Backend discovery 結果を見られる。

### 5. Change notification / invalidation を追加する

Profile source が更新されたとき、Backend 内の derived Profile state を invalidation する。

実装すること:

- Profile discovery cache がある場合は更新後に破棄・再構築する。
- Worker launch options API が更新後の Profile candidates を返す。
- 将来の ProfileSourceArchive builder が、更新後の source snapshot を使える状態にする。
- Runtime への直接 sync は本 Ticket の非目標とし、必要なら後続 Ticket に委譲する。

## 受け入れ条件

- Workspace-scoped Browser API で Workspace metadata を read/update できる。
- Workspace-scoped Browser API で Profile registry / Profile artifact を read/update できる。
- Frontend に Workspace settings page と Profiles settings page が追加されている。
- Profile source update 後、Backend の Profile discovery / launch options に変更が反映される。
- Profile selector duplicate、invalid registry schema、invalid Decodal syntax、path escape、symlink escape、artifact too large、concurrent update conflict は typed diagnostic になる。
- Browser-facing API / UI に host absolute path、secret value、Runtime endpoint、socket/session path、runtime-local store path が露出しない。
- Browser WorkingDirectory / Worker launch の profile candidates と settings page の profile list が同じ Backend discovery 結果を使う。
- `cargo test -p yoi-workspace-server` が通る。
- `cargo check -p yoi` が通る。
- `cd web/workspace && deno task check && deno task test` が通る。
- `yoi ticket doctor` が通る。
- `nix build .#yoi --no-link` が通る。

## 非目標

- Runtime が ProfileSourceArchive を prefetch/verify して Decodal Profile を resolve する経路の実装。
- Runtime-local plugin / MCP / sandbox artifact sync。
- Secret value editor。
- Multi-user auth / RBAC。
- Monaco 等の heavyweight code editor 導入。
- Ticket / Objective / Memory の編集画面統合。
