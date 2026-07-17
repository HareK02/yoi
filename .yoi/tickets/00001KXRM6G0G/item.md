---
title: 'Flat candidate staging record schemaを実装する'
state: 'ready'
created_at: '2026-07-17T18:07:40Z'
updated_at: '2026-07-17T18:10:00Z'
assignee: null
---

## 背景

旧 staging は extract run ごとの batch payload (`decisions` / `discussions` / `attempts` / `requests`) と entry-level source refs を前提にしていた。しかし、consolidation から見ると batch は session に紐いた不必要な集団になり、record ごとの discard / merge / promote / defer が曖昧になる。

新方針では、extract は記憶化を検討すべき candidate を kind taxonomy に基づいて切り出し、candidate ごとに flat staging record を作る。

```text
1 extract run = 0..N staging records
1 staging record = 1 candidate = 1 consolidation decision unit
```

旧 batch schema との互換性は不要。clear responsibility と flat records を優先する。

関連:

- Objective `00001KVJSMQXZ`
- `.yoi/objectives/00001KVJSMQXZ/memory-architecture-overview.md`
- Ticket `00001KXMK7YMC` — session-explore feature付きextract workerを実装する
- Ticket `00001KXMK846H` — StagingからMemory化する審査・剪定フローを実装する

## 実装要件

- flat staging record schema を実装する。
- 1 file / record は 1 candidate のみを表す。
- candidate kind は次に限定する。
  - `preference`
  - `working_assumption`
  - `constraint`
  - `decision`
  - `open_question`
  - `lesson`
- record は少なくとも次を表現できる。
  - `schema_version`
  - `id`
  - `extract_run_id`
  - record-level `source`
  - `kind`
  - `claim`
  - `why_useful`
  - `staleness` / invalidation hint
  - bounded `evidence[]`
  - `source_refs[]`
- evidence は extract が選んだ bounded snippets のみを保持する。
  - raw session log 全体や Overview 全体は保存しない。
  - raw tool result 全文は保存しない。
- `source_refs` は evidence id / evidence kind / entry range など host-resolved anchor を保持する。
- extract run ごとの batch payload を staging record として保存しない。
- consolidation input は flat records を record ごとに列挙する。
- 旧 batch schema の読み込み互換は不要。

## 非目標

- `session-explore` feature / tools はこの Ticket では実装しない。
- `stage_candidate` / `finish_extraction` tool はこの Ticket では実装しない。
- staging resolution / disposition はこの Ticket では実装しない。
- Overview projection を staging に保存しない。

## 受け入れ条件

- flat candidate staging record を serialize / deserialize できる。
- 1 staging file が 1 candidate だけを表す。
- candidate kind が上記 taxonomy に制限されている。
- bounded evidence snippets と source refs を保持できる。
- raw session log / Overview 全体 / raw tool result 全文を schema が要求しない。
- consolidation input が flat records を candidate 単位で表示できる。
- relevant tests が追加または更新されている。
- `cargo test -p memory` または該当 crate の同等テストが通る。
- code 変更として `nix build .#yoi` が通る。
