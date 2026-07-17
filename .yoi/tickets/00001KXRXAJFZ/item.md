---
title: 'Internal Worker runnerを実装する'
state: 'closed'
created_at: '2026-07-17T20:47:11Z'
updated_at: '2026-07-17T20:58:47Z'
assignee: null
queued_by: 'yoi ticket'
queued_at: '2026-07-17T20:47:48Z'
---

## 背景

Memory extract は runtime 側の Worker から派生する internal worker として動かす。今後 `session-explore` feature 付き extract worker、compaction、consolidation なども同じ形で安全に動かせるよう、Worker 内部に isolated internal worker / sub-worker runner の標準経路を用意する。

Protocol に外部 API として載せる段階ではない。まず Worker 内部 API として実装し、後で session 永続化、progress event、cancellation、Protocol exposure を追加できる形にする。

## 実装方針

- Worker が internal worker の起動を所有する。
- Feature は internal worker に渡す tool / context を contribute するだけで、Feature 自体が Worker を spawn しない。
- internal worker は foreground Worker history を汚染しない。
- prompt / input / tool registry / usage capture / output collection を `InternalWorkerSpec` 的な構造にまとめる。
- 初期実装では既存 memory extract sub-engine をこの runner に載せ替える。
- session 永続化はこの Ticket では実装しないが、runner の引数や result が後で persistence metadata を持てる形にする。

## 実装要件

- Worker 内部に reusable internal worker runner を追加する。
- runner は少なくとも次を受け取れる。
  - purpose / name。
  - system prompt。
  - initial user input。
  - tool definitions / limited tool registry。
  - optional usage capture。
- runner result は少なくとも次を返す。
  - run result。
  - captured usage。
  - finish/error metadata。
- foreground history / canonical session log に internal prompt や harness messages を混ぜない。
- existing memory extract path を runner 経由にする。
- existing extract behavior を変えない。
  - post-run threshold trigger のまま。
  - `write_extracted` transitional path のまま。
  - staging output は flat candidate records のまま。
- compaction / consolidation の載せ替えは非目標だが、後から同じ runner に寄せられる構造にする。

## 非目標

- Protocol method として external client から internal worker を起動できるようにしない。
- internal worker session persistence をこの Ticket で実装しない。
- `session-explore` feature や evidence tools はこの Ticket では実装しない。
- compaction / consolidation の full migration はこの Ticket では実装しない。

## 受け入れ条件

- reusable internal worker runner が存在する。
- memory extract がその runner 経由で動く。
- internal worker prompt/input が foreground history に永続化されない。
- tool surface は caller が渡した limited tool registry に閉じる。
- existing extract tests / memory tests / worker tests が通る。
- `cargo test -p memory` と `cargo test -p worker` が通る。
- code 変更として `nix build .#yoi` が通る。
