---
title: 'Add manual prune plan and cleanup operations for Workers and workdirs'
state: 'planning'
created_at: '2026-07-10T16:11:33Z'
updated_at: '2026-07-10T16:31:05Z'
assignee: null
---

## 背景

Ticket `00001KX6BPY7M` で Backend SQLite に Worker / Workdir registry と Worker-Workdir link を追加する。その後続として、停止済み Worker、Session / transcript、Workdir 実ファイル、removed / missing Workdir record を安全に整理する manual cleanup / prune の仕組みが必要になる。

削除方針は基本手動とする。Yoi が勝手に Worker history や workdir changes を削除しない。Runtime / Backend は削除候補と理由を plan として提示し、ユーザーまたは明示的な orchestration authority が実行した場合だけ削除する。

## 方針

- 削除は plan-first にする。
- 自動削除はこの Ticket の範囲外。期限切れや容量制限による automatic prune は後続判断とする。
- Worker は保存単位であり、Worker を archive/prune することは内包する Session / transcript retention と連動する。
- `pinned` Worker は削除候補にしてはならない。この Ticket で pinned flag の永続化と pin/unpin mutation API も明示的に実装する。
- Workdir 実ファイルは clean かつ stopped/archived Worker だけに紐づく場合、manual cleanup 候補にできる。
- Dirty Workdir は活動中/要判断状態として扱い、manual cleanup でも explicit discard confirmation が必要。
- Dirty orphan は recovery Worker 起動または explicit discard の判断対象であり、通常 prune とは区別する。
- Removed / missing Workdir record は、linked Worker archive が必要な summary を保持している場合だけ manual record prune 候補にできる。
- Raw Runtime path は Browser-facing plan に出さない。

## 要件

- Backend registry の Worker record に pinned flag または equivalent retention field を追加し、永続化する。
- Backend API に Worker pin / unpin mutation を追加する。mutation は Backend Worker registry を更新し、Runtime process には不要な副作用を起こさない。
- Backend registry を基準に Worker / Workdir / link の prune plan を生成する。
- Runtime observation を merge して、workdir files の状態を `present` / `removed` / `missing` / `unknown` 相当で表示する。
- Plan は対象ごとに、削除可能性、blocking reason、削除種別、推定 reclaim bytes を返す。
- Manual operation として少なくとも以下を表せる。
  - Worker archive
  - Worker history/session deletion または summary-only retention
  - Workdir files cleanup
  - Removed/missing Workdir record prune
  - cleanup_pending retry
- 実行 API は対象 ID と expected plan revision / digest を受け取り、古い plan に基づく削除を拒否する。
- `pinned` Worker とそれに紐づく Session / Workdir record は削除実行時にも保護される。
- Running Worker に紐づく Workdir files cleanup は拒否される。
- Dirty Workdir の discard は通常 cleanup とは別 action として明示される。
- Browser UI は plan preview を表示し、ユーザーが明示的に選択した対象だけ実行できる。

## 受け入れ条件

- `00001KX6BPY7M` の Backend Worker/Workdir registry と link model を前提として実装されている。
- Backend API で Worker を pin / unpin でき、状態が SQLite に永続化される。
- Worker list/detail は pinned state を返す。
- Backend API で Runtime ごとの manual prune plan を取得できる。
- Plan に Worker candidates と Workdir candidates が分かれて表示される。
- Candidate には削除種別、理由、blocking reason、linked Worker/Workdir ids、retention state、pinned state が含まれる。
- `pinned` Worker は prunable にならない。
- Running Worker に linked された Workdir files は cleanup candidate にならない。
- Clean + stopped/archived Worker only の Workdir files は cleanup candidate になる。
- Removed + no retained link の Workdir record は record prune candidate になる。
- Dirty orphan は normal prune candidate ではなく recovery/discard-required として表示される。
- Manual cleanup 実行 API は stale plan revision を拒否する。
- Manual cleanup 実行後、Backend registry と Runtime observation の状態が更新される。
- UI は Runtime > Workdirs / Workers 周辺から plan preview と manual execution に到達できる。
- Raw Runtime path が Browser-facing response / UI に出ない。
- `cargo test -p yoi-workspace-server --lib` が通る。
- `cargo test -p worker-runtime --features ws-server,fs-store` が通る。
- `cd web/workspace && deno task check && deno task test` が通る。
- `nix build .#yoi --no-link` が通る。

## 非目標

- 容量や期限による自動 prune policy は扱わない。
- Dirty Workdir を自動的に commit/push する recovery Worker 実装は扱わない。
- Session / transcript の高度な要約・圧縮は扱わない。削除または summary-only retention の hook までに留める。
- Runtime raw path を Backend canonical record に保存することはしない。
