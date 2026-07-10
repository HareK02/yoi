---
title: 'Add manual delete and cleanup operations for Workers and workdirs'
state: 'inprogress'
created_at: '2026-07-10T16:11:33Z'
updated_at: '2026-07-10T18:29:56Z'
assignee: null
queued_by: 'workspace-panel'
queued_at: '2026-07-10T16:45:14Z'
---

## 背景

Ticket `00001KX6BPY7M` で Backend SQLite に Worker / Workdir registry と Worker-Workdir link を追加する。その後続として、停止済み Worker、Session / transcript、Workdir 実ファイル、removed / missing Workdir record を安全に整理する manual delete / cleanup の仕組みが必要になる。

削除方針は基本手動とする。Yoi が勝手に Worker history や workdir changes を削除しない。Backend は削除候補と理由を plan として提示し、ユーザーまたは明示的な orchestration authority が実行した場合だけ削除する。

## 方針

- Worker lifecycle はシンプルに `running -> stopped -> delete` とする。`archived` を lifecycle state として導入しない。
- Worker は保存単位であり、Session / transcript は Worker に内包または参照される履歴として扱う。
- Worker delete は Worker record と内包する Session / transcript history の削除を意味する。Workdir は自動削除しない。
- `pinned` Worker は delete 候補にしてはならない。この Ticket で pinned flag の永続化と pin/unpin mutation API も明示的に実装する。
- Session / transcript の圧縮保存や高度な archive storage policy はこの Ticket では扱わない。必要なら後続で `history_storage: full | summarized | deleted` のような別概念として扱う。
- Workdir 実ファイルは durable history ではなく再現可能 cache として扱い、基本的に手動で削除可能にする。
- Clean Workdir は、running Worker に紐づいていなければ manual cleanup 候補にできる。
- Dirty Workdir は削除可能だが、changes ごと消す explicit confirmation を要求する。
- Dirty orphan は recovery Worker 起動または explicit discard の判断対象であり、通常の clean cleanup とは区別する。
- Removed / missing Workdir record は、linked Worker が最後の repository/selector/resolved commit summary を保持している場合、manual record delete 候補にできる。
- Raw Runtime path は Browser-facing plan に出さない。

## 要件

- Backend registry の Worker record に pinned flag または equivalent retention field を追加し、永続化する。
- Backend API に Worker pin / unpin mutation を追加する。mutation は Backend Worker registry を更新し、Runtime process には不要な副作用を起こさない。
- Backend registry を基準に Worker / Workdir / link の manual cleanup plan を生成する。
- Runtime observation を merge して、workdir files の状態を `present` / `removed` / `missing` / `unknown` 相当で表示する。
- Plan は対象ごとに、削除可能性、blocking reason、削除種別、推定 reclaim bytes を返す。
- Manual operation として少なくとも以下を表せる。
  - Worker delete（Worker record +内包 Session / transcript history）
  - Workdir files cleanup
  - Dirty Workdir discard with explicit confirmation
  - Removed/missing Workdir record delete
  - cleanup_pending retry
- 実行 API は対象 ID と expected plan revision / digest を受け取り、古い plan に基づく削除を拒否する。
- `pinned` Worker とそれに紐づく Session history は削除実行時にも保護される。
- Running Worker に紐づく Workdir files cleanup は拒否される。
- Dirty Workdir の discard は通常 cleanup とは別 action として明示される。
- Browser UI は plan preview を表示し、ユーザーが明示的に選択した対象だけ実行できる。

## 受け入れ条件

- `00001KX6BPY7M` の Backend Worker/Workdir registry と link model を前提として実装されている。
- Backend API で Worker を pin / unpin でき、状態が SQLite に永続化される。
- Worker list/detail は pinned state を返す。
- Backend API で Runtime ごとの manual cleanup plan を取得できる。
- Plan に Worker delete candidates と Workdir cleanup/delete candidates が分かれて表示される。
- Candidate には削除種別、理由、blocking reason、linked Worker/Workdir ids、pinned state が含まれる。
- `pinned` Worker は delete/prune candidate にならない。
- Running Worker に linked された Workdir files は cleanup candidate にならない。
- Clean + no running linked Worker の Workdir files は cleanup candidate になる。
- Dirty Workdir は discard confirmation required として表示され、通常 cleanup と区別される。
- Removed + no retained link の Workdir record は record delete candidate になる。
- Manual cleanup/delete 実行 API は stale plan revision を拒否する。
- Manual cleanup/delete 実行後、Backend registry と Runtime observation の状態が更新される。
- UI は Runtime > Workdirs / Workers 周辺から plan preview と manual execution に到達できる。
- Raw Runtime path が Browser-facing response / UI に出ない。
- `cargo test -p yoi-workspace-server --lib` が通る。
- `cargo test -p worker-runtime --features ws-server,fs-store` が通る。
- `cd web/workspace && deno task check && deno task test` が通る。
- `nix build .#yoi --no-link` が通る。

## 非目標

- 容量や期限による自動 prune policy は扱わない。
- `archived` Worker lifecycle state は導入しない。
- Session / transcript の圧縮 archive storage は扱わない。
- Dirty Workdir を自動的に commit/push する recovery Worker 実装は扱わない。
- Runtime raw path を Backend canonical record に保存することはしない。
