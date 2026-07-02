---
title: 'Workspace BrowserにSettings/Admin画面のshellとnavigationを追加する'
state: 'closed'
created_at: '2026-07-02T13:59:17Z'
updated_at: '2026-07-02T14:39:02Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-07-02T14:24:54Z'
---

## 背景

Workspace Browser には Worker / Runtime / Repository / Ticket などの作業 surface はあるが、Backend 設定や接続状態を扱う Settings/Admin surface はまだ無い。

今後 Runtime connections、Backend config、data store status、workspace identity などを UI から確認・編集する必要がある。これは普通に管理画面として設計する。ただし現状の Workspace Backend には user / permission / multi-user authorization が無いため、存在しない権限モデルを UI 上で fake しない。

Runtime connection 管理 UI を直接作り始めると、Settings/Admin の entry point / route / navigation / diagnostic / restart-required 表示などの共通設計と、Runtime connection config の永続化が混ざる。この Ticket では先に Settings/Admin shell と navigation だけを作り、Runtime Connections は後続 Ticket で実装する。

## 目的

- Workspace Browser に Settings/Admin surface を追加する。
- 将来の admin UI の受け皿になる route / layout / navigation を用意する。
- 現時点で存在しない user / permission / multi-user authorization を UI に出さない。
- Runtime Connections などの後続 Settings section を追加できるようにする。
- 既存の Worker Console / Sidebar UX を壊さない。
- mutation や Runtime connection 永続化は後続 Ticket に委譲する。

## 位置づけ

Settings/Admin は Workspace Backend の管理画面である。ただし v0 では権限管理を持たない。

明記する制約:

- user account は無い。
- role / permission model は無い。
- multi-user authorization は無い。
- したがって「管理者だけが操作できる」などの UI 文言や fake role を作らない。
- 現時点の操作境界は「この Backend にアクセスできること」だけである。

将来の受け皿:

- Runtime connections。
- Backend config / effective config view。
- data root / store status。
- workspace identity view。
- restart required / sanitized diagnostics。
- 将来の user / permission 管理。ただしこの Ticket では実装しない。

## UI entry point

Workspace Browser の sidebar に Settings/Admin entry point を追加する。

候補:

```text
...
SETTINGS
```

または bottom utility として:

```text
⚙ Settings
```

v0 では既存の Worker / Repository / Ticket navigation を邪魔しない位置に置く。Worker Console が主導線であることを崩さない。

## Route 設計

SvelteKit route として Settings/Admin shell を追加する。

候補:

```text
/settings
/settings/runtime-connections
/settings/backend-config
```

v0 では `/settings` が overview / placeholder を表示し、section navigation から placeholder section に遷移できるところまででよい。

Settings section の初期候補:

- Runtime Connections
  - 後続 Ticket `00001KWHHRTM9` で実装する。
  - v0 shell では placeholder / empty state を表示する。
- Backend Config
  - `.yoi/workspace-backend.local.toml` / packaged default / effective config の表示・diff の受け皿。
  - v0 shell では read-only placeholder でよい。
- Workspace Identity
  - workspace id / display name / initialized state などの read-only 表示の受け皿。

## 権限モデルについての表示

Settings/Admin overview には、現在 user / permission model が無いことを簡潔に示す。

例:

```text
This Workspace Backend currently has no user or permission model. Anyone with access to this backend can change these settings.
```

日本語 UI なら:

```text
現在、この Workspace Backend にはユーザー権限モデルがありません。この Backend にアクセスできる人は設定を変更できます。
```

これは「管理画面ではない」という意味ではない。無い権限モデルをあるように見せないための注意書きである。

## Common UI patterns

Settings/Admin shell では後続 section が使う共通 pattern を用意する。

### Sanitized diagnostic

- raw path / secret / token / socket path / runtime store path を出さない方針を示す。
- v0 は reusable component でなくても、Settings shell 内の表示 pattern として定義する。

### Restart required badge

Runtime connection や backend config の変更は Backend restart が必要になる場合がある。

v0 shell では badge / callout の見た目だけ用意してよい。

```text
Restart required
```

### Save / dirty state

この Ticket では mutation を実装しないため、dirty tracking の本実装は不要。ただし後続 section が使う前提として、save/cancel area を置ける layout にする。

## Backend API との関係

この Ticket では Settings mutation API は実装しない。

後続 Ticket では API path を次のどちらかに寄せることを検討する。

```http
GET /api/settings
GET /api/settings/runtime-connections
```

または Workspace 配下であることを強調するなら:

```http
GET /api/workspace/settings
GET /api/workspace/settings/runtime-connections
```

v0 shell は既存 `/api/workspace` projection などで表示可能な範囲に留めてよい。新 API を追加する場合も read-only summary に限定する。

## 依存関係

この Ticket は Runtime connection 管理 Ticket `00001KWHHRTM9` の前提とする。Runtime connection の add/delete/test/config persistence はこの Ticket では実装しない。

## 実装要件

- Workspace Browser に Settings/Admin entry point を追加する。
- `/settings` route を追加する。
- Settings/Admin shell layout を追加する。
- Section navigation を追加する。
- Runtime Connections placeholder section を追加する。
- Backend Config placeholder section を追加する。
- Workspace Identity placeholder / read-only section を追加する。
- 現時点で user / permission model が無いことを UI に明記する。
- user / role / permission の fake UI を作らない。
- Restart required / sanitized diagnostic の表示 pattern を用意する。
- 既存 Worker Console / Sidebar navigation と衝突しない。

## 受け入れ条件

- Sidebar または bottom utility から Settings/Admin に移動できる。
- `/settings` が表示できる。
- Settings/Admin shell に「現時点で user / permission model が無い」旨の注意書きがある。
- Settings section navigation がある。
- Runtime Connections placeholder がある。
- Backend Config placeholder がある。
- Workspace Identity placeholder または read-only summary がある。
- UI 文言が fake user / role / permission model を示唆しない。
- raw path / secret / token / socket path / runtime store path を placeholder や diagnostic に表示しない。
- 既存 Worker Console / Runtime Console / Sidebar の主導線が壊れていない。
- Focused tests が Settings route rendering、navigation、permission-model disclaimer、placeholder sections を確認する。
- `cd web/workspace && deno task test` が通る。
- `cd web/workspace && deno task check` が通る。
- `git diff --check` が通る。

## 対象外

- Runtime connection add/delete/test 実装。
- `.yoi/workspace-backend.local.toml` の read-modify-write API。
- Backend config editor。
- secret store UI。
- user / permission UI。
- Runtime live register / unregister。
- Manual Coding Worker 作成 form。
- Nix packaging に関わる変更。
