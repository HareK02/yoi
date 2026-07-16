---
title: 'session-explore feature付きextract workerを実装する'
state: 'planning'
created_at: '2026-07-16T04:34:01Z'
updated_at: '2026-07-16T17:19:41Z'
assignee: null
---

## 背景

Overview-first extract では、extract worker が開始時点の会話 snapshot と filtered Overview projection を参照し、必要な evidence だけを探索して staging payload を出す。main Worker の tool surface は増やさず、extract worker だけに session exploration 用の read-only tools を与える必要がある。

このため、`session-explore` feature を新設し、その feature のみが有効な Worker / ToolRegistry として extract worker を作る。extract worker は Memory / Knowledge / Skill / Ticket / docs を直接変更せず、`write_extracted` によって staging payload だけを出す。

この Ticket は worker 実装を扱う。先に Ticket `00001KXNYXNM6` で staging payload / source anchor 形式を固める。

関連:

- Objective `00001KVJSMQXZ`
- `.yoi/objectives/00001KVJSMQXZ/memory-architecture-overview.md`
- Ticket `00001KXMEZNYC` — ターン中のProgress messageを残す指示を追加する
- Ticket `00001KXNYXNM6` — Extract stagingのsource anchor形式を実装する

## 実装意図

この Ticket の目的は、extract worker に「何が起きたか」を flat log から推測させるのではなく、開始時点の immutable reference environment を探索させることである。

Reference environment:

```text
Conversation snapshot at extract start
  - extract 対象 history / segment range の immutable copy
  - message entries
  - tool call summaries
  - tool result summaries / bounded excerpts
  - source anchor table

Filtered Overview projection
  - User messages
  - Assistant text outputs, including Progress messages and final responses
  - committed task context
  - evidence ids への bounded index
```

Extract worker:

```text
Overview を読む
  -> important candidate を見つける
  -> session-explore tools で evidence を確認する
  -> host-resolved source anchors を使う
  -> write_extracted で staging payload を出す
```

この変更で目指す改善は recall をむやみに増やすことではない。断片的な `attempts` や無用な `discussions` を減らし、source に辿れる候補だけを staging に出す precision 改善を優先する。

## 対象領域

主な対象:

- `crates/worker/src/feature.rs`
  - `builtin:session-explore` feature descriptor / contribution registration の追加候補。
- `crates/memory/src/extract/input.rs`
  - Overview-first / Evidence-index-second rendering。
  - Conversation snapshot / filtered Overview projection の入力作成。
- `crates/memory/src/extract/tool.rs` または隣接 module
  - `write_extracted` に加えて session exploration tools を実装。
- `crates/memory/src/extract/payload.rs`
  - Ticket `00001KXNYXNM6` の staging source refs を利用。
- `crates/memory/src/extract/staging.rs`
  - staging write。
- `crates/worker/src/worker.rs`
  - `run_extract_once` / extract worker 起動。
  - extract worker 用 ToolRegistry / enabled feature set の構築。
- `resources/prompts/internal/memory_extract_system.md`
  - Overview-first / evidence-confirmation / staging-only output 前提へ更新。

## 実装方針

### 1. Reference environment を extract 開始時点で固定する

extract worker は live Worker state を自由に読むのではなく、extract 開始時点で host が作った immutable reference environment に接続する。

含めるもの:

- source range / segment id。
- Overview projection。
- Evidence index。
- evidence id -> source anchor table。
- bounded tool result summary / excerpt。

含めないもの:

- raw reasoning / chain-of-thought。
- unbounded raw tool result content。
- history に commit されていない hidden context injection。
- mutable live Worker state への直接 access。

### 2. `session-explore` feature を作る

`session-explore` は extract worker 専用の built-in feature とする。

Feature の tool surface:

- `search_evidence`
  - query / optional kind / optional limit で Evidence index を検索する。
  - output は evidence ids、entry ranges、short summaries、source anchors。
- `read_evidence`
  - evidence id または entry range を読む。
  - output は bounded message/tool summary/excerpt と source anchor。
- `resolve_source_anchor`
  - evidence id / entry range / tool call id から normalized source anchor を返す。
  - 実装上過剰なら、`search_evidence` / `read_evidence` output に anchor を含める形でもよい。
- `write_extracted`
  - structured staging payload を 1 回提出する output tool。

制約:

- read-only。
- file write / Ticket mutation / Memory direct write / Knowledge direct write / Skill direct write を持たせない。
- main Worker の通常 tool surface には出さない。
- extract worker の enabled feature set は `session-explore` のみにする。

### 3. Extract worker は専用 Worker / Engine として起動する

既存の memory extract sub-engine 起動を、`session-explore` feature のみ有効な worker-like environment に寄せる。

要件:

- foreground Worker history を汚染しない。
- extract prompt / harness messages は canonical foreground history に混ざらない。
- output は `write_extracted` 経由で host context に戻す。
- empty / NOP は正常結果。
- worker が evidence tools 以外の通常 workspace tools を見ないことを test する。

### 4. Trigger は広げすぎない

初期実装では既存 post-run trigger を維持してよい。LLM call ごとの extract はしない。

この Ticket の中心は scheduler ではなく、extract worker の参照環境と tool surface である。mid-run trigger や Overview accumulation trigger が大きくなる場合は別 Ticket に分ける。

## 実装要件

- extract 開始時点の conversation snapshot と filtered Overview projection を保持する reference environment を実装する。
- `builtin:session-explore` feature を追加する。
- extract worker は `session-explore` feature のみ有効な Worker / ToolRegistry として起動する。
- `session-explore` feature に bounded read-only evidence tools を実装する。
- `write_extracted` は output tool として維持する。
- `build_extract_input` 相当の入力を Overview-first / Evidence-index-second に変更する。
- extract worker は Ticket `00001KXNYXNM6` で定義された source anchor / evidence ref 形式を使って staging payload を出す。
- extract worker は Memory / Knowledge / Skill / Ticket / docs を直接変更しない。
- main Worker の model-visible tool surface は増やさない。
- raw tool result content 全文を無制限に extract prompt / tool output へ流さない。
- `memory_extract_system.md` を session-explore / evidence-confirmation / staging-only output 前提に更新する。

## 非目標

- staging payload / source anchor format の確定はこの Ticket では行わない。Ticket `00001KXNYXNM6` が先行する。
- staging -> Memory disposition / resolution flow はこの Ticket では実装しない。Ticket `00001KXMK846H` の対象。
- 専用 Progress Tool を main Worker に追加しない。
- Knowledge / Skill / docs を extract worker が直接 rewrite しない。
- 大規模な mid-run extraction scheduler はこの Ticket では作らない。

## 受け入れ条件

- extract 開始時点の immutable reference environment が作られる。
- filtered Overview projection が User messages / Assistant text outputs / committed task context から作られる。
- `builtin:session-explore` feature が存在し、extract worker にだけ有効化される。
- extract worker が `session-explore` の read-only evidence tools と `write_extracted` だけを使える。
- main Worker の model-visible tool surface が増えていない。
- extract output は staging にだけ書かれ、Memory / Knowledge / Skill への direct write がない。
- source / provenance は Ticket `00001KXNYXNM6` の形式で機械的に保持される。
- empty payload / no-op path が維持されている。
- Overview rendering、reference environment immutability、feature-gated tool registry、evidence tool bounds の tests が追加または更新されている。
- `cargo test -p memory` と関連 worker tests、または該当 crate の同等テストが通る。
- prompt/resource/code 変更として `nix build .#yoi` が通る。
