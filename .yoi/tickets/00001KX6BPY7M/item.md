---
title: 'Add Backend Worker/Workdir registry and link model'
state: 'inprogress'
created_at: '2026-07-10T15:53:02Z'
updated_at: '2026-07-10T17:31:13Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-07-10T16:10:57Z'
---

## 背景

Objective `00001KWW44EXK` の Worker / Session / workdir retention 方針に基づき、Worker と workdir の保存・retention・link authority を Backend SQLite に置く。

現在は Runtime が Worker catalog と working directory materialization record を主に持ち、Backend は Worker create / workdir create を仲介しているだけである。このため、停止済み Worker、Session / transcript、Runtime-owned workdir、実ファイルが削除済みの workdir、unknown/external workdir の関係を Backend が安定して説明できない。

今後の prune / archive / pinned retention を安全に実装するには、先に Backend 側に canonical registry と relation を持たせる必要がある。

## 決定事項

- Worker を作業記録の保存単位にする。
- Session / transcript は Worker archive に内包または参照される履歴として扱い、Session 単体を長期保存 authority にしない。
- Worker archive には retention policy を持たせ、`pinned` Worker を cleanup / prune から守れるようにする。
- Workdir record は Backend SQLite にも canonical record を持たせる。Runtime は実ファイル、raw materialized path、process/cwd binding、cleanup 実行の authority を持つ。
- Worker と Workdir の関係は SQLite link table で表す。Worker 側 summary copy は許容するが、relation authority は link table とする。
- Workdir 実ファイルは durable history ではなく再現可能 cache として扱う。
- Dirty workdir は活動中状態として扱う。Dirty orphan は自動 prune ではなく recovery Worker の起動または explicit discard の判断対象にする。
- Browser-facing UI 表示は `workdir` に寄せる。内部 API の `working_directory` 互換名は段階移行中に残ってよい。

## 要件

- Backend SQLite に Worker registry を追加する。
  - workspace_id
  - backend worker id / runtime worker id
  - runtime_id
  - display name
  - profile
  - lifecycle state
  - retention state（少なくとも pinned を表せること）
  - session/transcript refs または将来 refs を保存できる shape
  - summary/diagnostics 用の将来拡張余地
- Backend SQLite に Workdir registry を追加する。
  - workspace_id
  - workdir_id
  - runtime_id
  - repository_id
  - selector
  - resolved_commit
  - materialization status（present/removed/missing/unknown 相当）
  - cleanliness（clean/dirty/unknown 相当）
  - created_at / updated_at
- Backend SQLite に Worker-Workdir link table を追加する。
  - worker_id
  - workdir_id
  - role（primary など将来拡張可能）
  - linked_at / unlinked_at
- Backend 経由の workdir create は Backend Workdir registry row を作成/更新してから Runtime materialization を要求する。
- Backend 経由の worker create は Backend Worker registry row と Worker-Workdir link を作成/更新する。
- Runtime から返る Worker / Workdir status は Backend registry に同期される。
- Browser-facing Worker / Workdir list は Backend registry を主にし、Runtime status は観測値として merge する。
- Runtime direct API などで作られた Backend registry に無い workdir は unmanaged/external として扱い、通常の Backend-managed Workdirs 一覧に混ぜない。
- Raw Runtime path は Backend registry / Browser response に保存・表示しない。

## 受け入れ条件

- Backend SQLite schema に Worker / Workdir / Worker-Workdir link の typed tables が追加されている。
- 既存 store からの migration / default path があり、既存 Workspace Backend 起動を壊さない。
- Backend API 経由で workdir を作成すると、Runtime record だけでなく Backend Workdir registry にも record が残る。
- Backend API 経由で Worker を作成すると、Backend Worker registry と Worker-Workdir link が残る。
- Stopped Worker と removed Workdir の関係を Backend registry から辿れる。
- Worker archive / retention metadata に `pinned` を表せる。
- Workdir 実ファイルが無いことは Runtime observation として扱われ、Backend canonical record が即座に失われない。
- Backend-managed Workdir と unmanaged/external Runtime Workdir を区別できる。
- Workers / Workdirs UI は Backend-managed registry を基準に表示できる。
- `cargo test -p yoi-workspace-server --lib` が通る。
- `cargo test -p worker-runtime --features ws-server,fs-store` が通る。
- `cd web/workspace && deno task check && deno task test` が通る。
- `nix build .#yoi --no-link` が通る。

## 非目標

- 実際の prune/delete 実行はこの Ticket では扱わない。
- Session/transcript full deletion UI はこの Ticket では扱わない。
- Dirty orphan recovery Worker の自動起動はこの Ticket では扱わない。
- Runtime raw path を Backend canonical record に保存することはしない。
