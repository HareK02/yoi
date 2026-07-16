---
title: 'Overview-first extract と evidence 探索を実装する'
state: 'ready'
created_at: '2026-07-16T04:34:01Z'
updated_at: '2026-07-16T14:06:41Z'
assignee: null
---

## 背景

現状の Memory extract は conversation slice を flat Markdown として渡し、extract worker が `decisions` / `discussions` / `attempts` / `requests` を `write_extracted` で返す。tool call 名と tool result summary は含まれるが、User / Assistant の semantic flow と tool evidence が同列に並ぶため、目的・判断・現在地が抜けた断片的な抽出になりやすい。

この Ticket では設計だけで止めず、実装として extract 入力を Overview-first に変更し、extract worker が bounded read-only evidence tools で必要箇所を探索できるようにする。extract output は引き続き staging に限定し、Memory / Knowledge / Skill を直接更新しない。

関連:

- Objective `00001KVJSMQXZ`
- `.yoi/objectives/00001KVJSMQXZ/memory-architecture-overview.md`
- Ticket `00001KXMEZNYC` — ターン中のProgress messageを残す指示を追加する

## 実装意図

この Ticket の目的は、extract worker に「何が起きたか」を flat log から推測させるのではなく、main Worker が残した user-facing transcript を作業の地図として渡し、必要な evidence だけを確認させることである。

方針:

```text
User / Assistant messages
  -> Overview: 目的・現在地・判断・未解決点を追う semantic backbone

Tool calls / tool results
  -> Evidence index: Overview の claim を確認する根拠への bounded pointer

extract worker
  -> Overview を読んで重要そうな候補を見つける
  -> read-only evidence tools で必要範囲だけ確認する
  -> write_extracted で staging payload を出す
```

Progress message は専用 Tool ではなく通常 Assistant Message として残す方針なので、この Ticket では Progress message を新しい item kind として導入しない。extract 側は、committed history 上の User / Assistant messages を信頼できる overview source として扱い、tool evidence はそれを検証・補足するために使う。

この変更で目指す改善は recall をむやみに増やすことではない。むしろ、断片的な `attempts` や無用な `discussions` を減らし、source に辿れる候補だけを staging に出す precision 改善を優先する。

## 対象領域

主な対象は memory extract の入力構築、extract worker の tool surface、staging source 付与である。

想定される主な参照先:

- `crates/memory/src/extract/input.rs`
  - 現在の flat renderer。
  - Overview-first / Evidence-index-second rendering の中心候補。
- `crates/memory/src/extract/tool.rs`
  - `write_extracted` 実装。
  - evidence tools を追加する場合は、この module を分割するか、隣接 module に追加する。
- `crates/memory/src/extract/payload.rs`
  - `ExtractedPayload` / `StagingRecord`。
  - entry-level source anchors が必要なら、互換性を壊さない拡張をここで検討する。
- `crates/memory/src/extract/staging.rs`
  - staging 書き込み。
  - source anchor の保存形式と関係する。
- `crates/worker/src/worker.rs`
  - `run_extract_once` / extract worker 起動 / source range 付与 / post-run trigger。
  - extract 専用 ToolRegistry に evidence tools を登録する場所の候補。
- `resources/prompts/internal/memory_extract_system.md`
  - Overview を first-class input とし、evidence tools を使って根拠確認してから `write_extracted` する指示へ更新する。

必要なら周辺 crate / module に小さい helper を追加してよいが、main Worker の通常 tool registry に evidence tools を露出しないこと。

## 実装方針

### 1. まず Overview / Evidence index を host 側で作る

最初の実装では、extract worker に巨大な自由探索をさせない。Worker が渡す conversation range から、host 側で次を組み立てる。

- Overview
  - User messages。
  - Assistant messages。
  - Progress message と final response は同じ Assistant message として扱う。
  - System messages は原則 Overview の本文には混ぜない。必要な task context は committed history 由来に限定する。
- Evidence index
  - tool call id / tool name / entry index。
  - tool result summary / entry index。
  - raw content がある場合も全文は入れず、bounded excerpt または `read_evidence` で読める pointer にする。
  - reasoning は入れない。

extract input の top-level は、flat log ではなく次のような形にする。

```text
## Overview
...User / Assistant messages...

## Evidence index
- E001: ToolCall Read at entry 42
- E002: ToolResult summary at entry 43
...

## Instructions
Use Overview as the semantic guide. Use evidence tools only when needed. Call write_extracted once.
```

### 2. Evidence tools は extract slice の中だけを読む

初期実装の evidence tools は、まず extract 対象 slice 内に閉じてよい。repository filesystem や Ticket backend を直接探索する必要はない。

最低限の tool shape:

- `search_evidence`
  - input: query / optional kind / optional limit。
  - output: matching evidence IDs、entry ranges、short summaries。
- `read_evidence`
  - input: evidence ID または entry range。
  - output: bounded message/tool summary/excerpt。
- `resolve_source_anchor`
  - input: evidence ID / entry range / tool call id。
  - output: staging に保存できる normalized source anchor。

実装上 `resolve_source_anchor` が過剰なら、`search_evidence` / `read_evidence` の output に normalized anchor を含める形でもよい。重要なのは、extract worker が「根拠を読んだふり」をせず、host が解決した anchor を payload / staging に残せること。

### 3. Source anchor は段階的に拡張する

現状は `StagingRecord` 全体に `source` が付く。これを一気に壊さない。

推奨順:

1. 既存 `source` は維持する。
2. `ExtractedPayload` entry に optional な evidence refs / source anchors を追加できるか検討する。
3. 互換性が難しければ、staging record に `anchors` / `evidence_refs` の sibling field を追加する。
4. consolidation が未対応でも壊れないよう、unknown/optional field として扱える形にする。

entry-level provenance が最終目標だが、この Ticket では少なくとも evidence ID と source range を staging から辿れる状態にする。

### 4. Trigger は広げすぎない

初期実装では既存 post-run trigger を維持してよい。LLM call ごとの extract はしない。

この Ticket の中心は trigger scheduler ではなく、extract の入力品質と evidence 確認である。mid-run trigger や Overview accumulation trigger は、既存 threshold と矛盾しない小さな変更に留める。大きな scheduler 変更が必要なら別 Ticket に分ける。

## 実装要件

- `build_extract_input` 相当の入力を Overview-first / Evidence-index-second の形に変更する。
  - User messages と通常 Assistant messages を primary Overview として先に提示する。
  - Progress message / final response / user correction / approval を semantic guide として扱う。
  - tool calls / tool results は primary narrative ではなく Evidence index として提示する。
  - reasoning と raw tool result content 全文は引き続き入れない。
- extract worker 専用の constrained read-only evidence tools を実装する。
  - Evidence search: session slice / message index / tool summaries から候補 range を探す。
  - Evidence read: bounded session entry range、tool call/result summary、許可された bounded excerpt を読む。
  - Source anchor resolver: session id、entry range、tool call id、file path、Ticket/Objective artifact ref などを staging source に結びつける。
  - `write_extracted` は output tool として維持する。
- evidence tools は extract 専用 worker にだけ登録し、main Worker の model-visible tool surface は増やさない。
- extract worker は Memory / Knowledge / Skill / Ticket / docs を直接変更しない。
- extract output は staging に限定する。
- `ExtractedPayload` の entry と source anchors の結びつけを実装する。
  - 既存の staging record 全体 source だけで足りない場合、互換性を壊さない拡張にする。
- trigger は LLM call 単位にしない。
  - 初期実装では既存 post-run trigger を維持してよい。
  - Overview accumulation / Evidence growth / Worker run cycle / task boundary に基づく予約が必要なら、既存 threshold と矛盾しない形で導入する。
  - mid-run 発火を入れる場合も staging/checkpoint extraction に限定する。
- empty / NOP は正常結果として扱う。

## 非目標

- 専用 Progress Tool を main Worker に追加しない。
- extract worker に write 権限や durable resource mutation 権限を渡さない。
- Knowledge / Skill / docs を extract worker が直接 rewrite しない。
- raw tool result 全文を無制限に extract prompt へ流さない。
- この Ticket で staging -> Memory disposition / resolution flow まで実装しない。それは Ticket `00001KXMK846H` の対象。
- この Ticket で大規模な scheduler / mid-run extraction framework を作らない。

## 実装順序の目安

1. `extract/input.rs` に Overview / Evidence index の renderer を追加し、既存 tests を更新する。
2. extract 対象 slice から evidence index を作る host-side data structure を追加する。
3. read-only evidence tools を extract worker context に実装する。
4. `worker.rs` の extract worker 起動箇所で、`write_extracted` と evidence tools を extract 専用 registry に登録する。
5. `memory_extract_system.md` を Overview-first / evidence-confirmation 前提に更新する。
6. source anchor / evidence refs を staging output から辿れる形にする。
7. tests を追加し、raw tool content が無制限に入らないこと、main Worker tool surface が増えないこと、empty payload が維持されることを確認する。

## 受け入れ条件

- extract input が Overview-first / Evidence-index-second の形で生成される。
- extract worker が bounded read-only evidence tools を使って必要箇所を探索できる。
- main Worker の model-visible tool surface が増えていない。
- extract output は staging にだけ書かれ、Memory / Knowledge / Skill への direct write がない。
- source / provenance が機械的に保持され、extract worker が根拠を推測しない実装になっている。
- 既存の empty payload / no-op path が維持されている。
- Overview-first rendering、Evidence index、evidence tool bounds、source anchor について unit tests が追加または更新されている。
- `cargo test -p memory` または該当 crate の同等テストが通る。
- prompt/resource/code 変更として `nix build .#yoi` が通る。
