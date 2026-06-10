---
title: 'Lua Profileに組み込みyoi APIとimport/extendを追加する'
state: 'inprogress'
created_at: '2026-06-10T07:19:31Z'
updated_at: '2026-06-10T09:36:53Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-06-10T08:04:05Z'
---

## 背景

Lua Profile では現在、標準 API を使うたびに `require("yoi.profile")` / `require("yoi.scope")` / `require("yoi.compact")` などを明示している。標準 API は sandbox 内で host が与える組み込み surface なので、Profile 実行時に最初から `yoi` object を global として注入し、すべて `yoi.*` 経由でアクセスできる形に整理したい。

また、builtin Profile は registry selector としては `builtin:default` を選べるが、Lua Profile 内から import して上書き・再 export する標準経路がない。project-local の `.yoi/profiles/_base.lua` に function を置いて各 Profile から `require("_base")` する運用はできるが、builtin base Profile を再利用する API ではない。

## 要件

- Lua Profile 実行環境に global `yoi` object を注入する。
- 標準 API は `yoi.profile` / `yoi.scope` / `yoi.compact` / `yoi.models` のように、単一の `yoi` object 配下から利用できる。
- `yoi.profile` は `yoi.profile { ... }` と呼べる callable な表現にしつつ、Profile 操作用 helper を持てる形にする。
- builtin / registry Profile を Lua Profile 内から reusable Profile artifact として import できる API を追加する。
  - 例: `yoi.profile.import("builtin:default")`
  - import 対象は resolved Manifest ではなく、workspace/runtime 解決前の raw Profile artifact とする。
- import した Profile artifact に対して override を適用し、Profile として再 export できる API を追加する。
  - 例: `return yoi.profile.extend("builtin:default", { slug = "coder", worker = { language = "Japanese" } })`
  - 例: `local base = yoi.profile.import("builtin:default"); return yoi.profile.extend(base, { ... })`
- merge semantics を明示して実装する。
  - object/table は再帰的に merge する。
  - scalar は override する。
  - array/list は replace する。
  - `nil` による削除 semantics はこのチケットでは導入しない。
- import/extend 後の最終 artifact に対して、既存の Profile validation を維持する。
  - `pod`、complete Manifest 形状、concrete `scope.allow` / `scope.deny`、resolved absolute path などは禁止のまま。
- bundled `resources/profiles/default.lua` は `require("yoi.*")` ではなく global `yoi` style に更新する。
- project-local module composition のための local `require("...")` は維持する。
- `require("yoi")` / `require("yoi.*")` の扱いを整理する。
  - 互換 alias として当面残すか、明示的に非推奨化するかを実装時に判断し、テストで固定する。

## 受け入れ条件

- `resources/profiles/default.lua` が `local profile = require("yoi.profile")` などを使わず、global `yoi` だけで評価できる。
- filesystem Profile でも embedded builtin Profile でも、global `yoi` API が利用できる。
- Lua Profile で `yoi.profile.import("builtin:default")` または同等の API により builtin default の raw Profile artifact を取得できる。
- Lua Profile で `yoi.profile.extend("builtin:default", overrides)` または同等の API により builtin default を上書きした Profile を返せる。
- override は nested table を意図通り deep merge し、scalar と list は置換される。
- import/extend で runtime-bound field や concrete authority を混入させた場合、既存 Profile 境界に沿って reject される。
- project-local `require("_base")` などは引き続き動作する。
- 新 API と既存 default Profile の評価に対する manifest crate のテストが追加・更新されている。
- `cargo test -p manifest profile` または該当する targeted test が通る。
- `target/debug/yoi ticket doctor` が通る。

## 非目標

- resolved Manifest を Lua Profile へ import 可能にすること。
- runtime-bound な `pod.name` や concrete `scope.allow` / `scope.deny` を Profile に許可すること。
- local filesystem require を builtin embedded profile の module search path と混ぜること。
- `nil` / tombstone による field deletion API の導入。
