---
title: 'Session reference viewを共通基盤として実装する'
state: 'ready'
created_at: '2026-07-17T23:04:40Z'
updated_at: '2026-07-17T23:20:00Z'
assignee: null
---

## 背景

Memory extract は runtime 側で session snapshot から filtered view を作り、`session-explore` tools で evidence を読んで flat staging candidates を作る予定である。一方、compaction も既に session history から summary input を作り、`search_session_log` / `read_session_items` で詳細を探索する形に近い。

この2つは目的が違うが、共通して「session history から人間/LLMが扱いやすい参照 view を作る」「bounded に探索・読取する」という基盤を必要とする。extract 専用でも compact 専用でもない中立的な共通基盤として `Session reference view` を実装する。

関連:

- Objective `00001KVJSMQXZ`
- Ticket `00001KXMK7YMC` — session-explore feature付きextract workerを実装する
- worker compaction implementation: `crates/worker/src/compact/worker.rs`

## 実装意図

`Session reference view` は、session history の full raw dump ではなく、internal worker が必要な範囲を辿るための host-created immutable view である。

目的:

- User / Assistant text outputs から semantic overview を作る。
- Tool calls / tool results / file refs などを evidence index として分離する。
- compact / extract などの internal worker が同じ read/search substrate を使えるようにする。
- read/search output を bounded にし、raw tool result 全文や reasoning を無制限に渡さない。
- extract では source anchor / evidence id を staging record へ接続できるようにする。
- compact では summary worker が必要な session detail を安全に読めるようにする。

## 対象領域

候補:

- `crates/worker/src/compact/worker.rs`
  - 既存の `build_summary_input` / `search_session_log` / `read_session_items` に相当する projection / read 機能。
- `crates/memory/src/extract/input.rs`
  - extract 用 Overview-first input。
- `crates/worker/src/internal_worker.rs`
  - internal worker runner から参照 view を渡す将来接続点。
- 新規 module 候補:
  - `crates/worker/src/session_reference.rs`
  - または extract/compact から独立した crate-local module。

## 実装要件

- immutable `SessionReferenceView` 相当の型を実装する。
  - view 作成時点の session items / `entry_range` を固定する。
  - foreground history の live mutation を直接読まない。
- view は少なくとも次を提供する。
  - overview projection。
  - evidence index。
  - bounded search。
  - bounded read by stable id / `entry_range`。
- overview projection は User / Assistant text outputs を主軸にする。
  - Progress message と final response は同じ text output として扱う。
  - tool result 全文や reasoning は overview に混ぜない。
- evidence index は user / assistant / system / tool などの session reference kind で参照可能にする。
  - evidence id を持つ。
  - `entry_range` を持つ。
  - kind を持つ。
  - tool entry は input / output のどちらを対象にするか区別できる。
  - short label / summary を持つ。
- bounded read は、指定範囲の message / tool summary / bounded excerpt を返す。
  - token / byte / item count の上限を持つ。
  - over-limit 時は truncation metadata を返す。
- search は初期実装では text match / substring / simple case-insensitive query でよい。
  - jq-like structural query は初期実装に含めない。
  - ただし kind / id / `entry_range` で絞れる API にする。
  - tool kind では input / output / both を検索対象として指定できる。
- extract 向けに、evidence id から source anchor を作れる情報を保持する。
- compact 向けに、既存 `search_session_log` / `read_session_items` 相当を置き換えられる構造にする。
- 既存 compaction behavior を壊さない。

## Access model 方針

初期 API は、LLM が扱いやすい **id / `entry_range` based access** を中心にする。

- Overview item には `overview_id` / `message_seq` のような安定 id を付ける。
- Progress message だけを特別扱いして `prog y` で指定するより、User / Assistant text output を同じ overview item として並べる。
- ただし表示上は、長い turn の中で progress が複数あることが分かるように label を持たせる。
- 範囲指定は `overview_id` / `evidence_id` / `entry_range` を基本にする。
- `turn x, prog y ~ turn x, prog z` のような UI 表示は可能だが、tool input の canonical key にはしない。
- `kind` は role と evidence type を統合した filter として扱う。
  - `user`
  - `assistant`
  - `system`
  - `tool`
- tool kind では `tool_part` を指定できる。
  - `input`: tool call name / arguments を検索・読取対象にする。
  - `output`: tool result summary / bounded content excerpt を検索・読取対象にする。
  - `both`: input / output の両方を対象にする。

推奨 tool/API shape:

```text
search_reference(query, kind?, tool_part?, limit?) -> ids + labels + entry_ranges
read_reference(id | entry_range, include_tools?, tool_part?, max_items?, max_bytes?) -> bounded entries + truncation metadata
resolve_reference(id | entry_range) -> source anchor
```

探索は初期段階では text search と typed filters の組み合わせにする。jq-like structural query は強力だが、LLM tool input として複雑になりやすいため後回しにする。tool input / output を別々に検索したい需要は初期 filter として扱う。

## 非目標

- Protocol method として external client に公開しない。
- vector search / embedding search は実装しない。
- jq-like full structural query language は実装しない。
- raw tool result 全文を unbounded に読ませない。
- Progress message 専用の別 item kind を導入しない。
- compaction の全面 rewrite はこの Ticket では行わない。
- `session-explore` feature tools 本体はこの Ticket では実装しない。共通基盤だけを作る。

## 受け入れ条件

- `SessionReferenceView` 相当の共通型がある。
- User / Assistant text outputs から overview projection を作れる。
- tool calls / tool results を evidence index として参照できる。
- search filter は `kind` で user / assistant / system / tool を扱える。
- tool kind では input / output / both を検索・読取対象として選べる。
- bounded search / read がある。
- read output に truncation metadata がある。
- extract が source anchor を作るための evidence id / entry range / kind を取得できる。
- compact 側の既存 search/read と置き換え可能な形になっている。
- relevant unit tests が追加されている。
- `cargo test -p worker` または該当 crate の同等テストが通る。
- code 変更として `nix build .#yoi` が通る。
