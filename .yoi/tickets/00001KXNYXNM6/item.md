---
title: 'Extract stagingのsource anchor形式を実装する'
state: 'ready'
created_at: '2026-07-16T17:17:22Z'
updated_at: '2026-07-16T17:19:41Z'
assignee: null
---

## 背景

Overview-first extract では、extract worker が開始時点の会話 snapshot と filtered Overview projection を参照し、必要な evidence だけを探索して staging payload を出す。これを安全に実装するには、先に staging payload / source anchor / evidence reference の形式を固める必要がある。

現状の `StagingRecord` は record 全体に `source: SourceRef` を持つが、個々の `decision` / `discussion` / `attempt` / `request` がどの evidence に基づくかを表現しづらい。consolidation が staging を審査・剪定するためには、entry-level source anchors と overview/evidence refs を辿れる形式が必要である。

関連:

- Objective `00001KVJSMQXZ`
- `.yoi/objectives/00001KVJSMQXZ/memory-architecture-overview.md`
- Ticket `00001KXMK7YMC` — `session-explore` feature 付き extract worker 実装
- Ticket `00001KXMK846H` — staging から Memory 化する resolution / disposition 実装

## 実装意図

この Ticket は `session-explore` worker の前提となる staging schema を実装する。目的は、extract worker が「根拠を読んだふり」をせず、host が解決した source anchor と staging entry を機械的に結びつけられるようにすることである。

この Ticket では extract worker の feature / tool 実装までは行わない。まず serialization / schema / tests を固める。

## 対象領域

主な対象:

- `crates/memory/src/extract/payload.rs`
  - `ExtractedPayload`
  - `DecisionEntry` / `DiscussionEntry` / `AttemptEntry` / `RequestEntry`
  - entry-level source refs / evidence refs の追加
- `crates/memory/src/extract/staging.rs`
  - staging write / read compatibility
- `crates/memory/src/schema/common.rs`
  - `SourceRef` との関係確認
- `crates/memory/src/consolidate/input.rs`
  - 新形式 staging を consolidation input に表示できるか確認
- relevant tests under `crates/memory`

## 実装要件

- extract staging の source anchor / evidence reference 型を実装する。
  - session id / segment id。
  - entry range。
  - evidence id。
  - evidence kind: message / tool_call / tool_result / file_ref / ticket_ref / objective_ref など拡張可能な形。
  - optional summary / label。
- `DecisionEntry` / `DiscussionEntry` / `AttemptEntry` / `RequestEntry` に optional entry-level source refs を持たせる。
  - 既存 JSON 互換性を壊さないため default empty / skip serializing empty とする。
- 既存の `StagingRecord.source` は維持する。
  - record-level source は extract 対象 range 全体を示す。
  - entry-level source refs は個々の claim の evidence を示す。
- source refs は extract worker が自由作文した根拠文字列ではなく、host が解決した anchor を参照できる形式にする。
- staging JSON の読み書きで、旧形式 staging が読めることを維持する。
- consolidation input で entry-level source refs が確認できるようにする。
  - 最初は human-readable 表示でよい。
  - consolidation が未対応でも壊れない optional field として扱う。
- raw tool result content 全文を staging payload に埋め込まない。

## 非目標

- `session-explore` feature / evidence tools をこの Ticket で実装しない。
- extract worker 起動経路をこの Ticket で変更しない。
- staging -> Memory resolution / disposition log をこの Ticket で実装しない。
- Knowledge / Skill / Ticket / docs への routing をこの Ticket で実装しない。

## 受け入れ条件

- staging payload に entry-level source refs / evidence refs を保存できる。
- 既存 `StagingRecord.source` と互換性が維持されている。
- 旧形式 staging JSON が読める。
- new-format staging JSON が serialization / deserialization できる。
- consolidation input に source refs が表示される、または少なくとも lossless に通る。
- source refs は bounded metadata / anchors のみで、raw tool result 全文を含まない。
- relevant unit tests が追加または更新されている。
- `cargo test -p memory` または該当 crate の同等テストが通る。
- code 変更として `nix build .#yoi` が通る。
