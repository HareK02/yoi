---
title: 'TUI Console: 連続した Thinking block を一つの表示グループにまとめる'
state: 'inprogress'
created_at: '2026-06-24T11:39:59Z'
updated_at: '2026-06-24T12:14:38Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['tui-rendering', 'reasoning-display', 'block-aggregation', 'text-selection']
queued_by: 'workspace-panel'
queued_at: '2026-06-24T12:01:41Z'
---

## User claims / request snapshot

- ユーザーは、TUI Console で連続した `Thinking` を `Read` tool 表示のように一つにまとめて表示したいと依頼した。
- 対象 workspace は `yoi`。
- Panel handoff の orchestrator Pod は `yoi-orchestrator`。

## Confirmed facts / sources

- 既存 Ticket 確認では、同じ目的の active duplicate は見当たらなかった。
- 関連 closed Ticket:
  - `00001KT5D44Z0` — reasoning / thinking persistence を block lifecycle に統一済み。
  - `00001KVMT2J25` — in-flight snapshot が unfinished thinking block を含め、TUI が snapshot から unfinished thinking を seed して live delta 継続できるようにした。
  - `00001KVHX0WBE` — Dashboard / Console / TUI の呼称と module boundary を整理済み。今回の対象は Console / single-Pod chat surface。
  - `00001KTKCK8W8` — chat view の Markdown rendering 改善。今回と同じく表示層の変更であり、history / context 変更は避ける方針が近い。
- `00001KSVP63K8` は planning の in-flight composer injection で、今回の Thinking 表示集約とは別件。
- `crates/tui/src/block.rs` では `Block::Thinking(ThinkingBlock)` が独立した history block として存在し、`ThinkingState::{Streaming, Finished, Incomplete}` を持つ。
- `crates/tui/src/app.rs` では `Event::ThinkingStart` ごとに新しい `Block::Thinking` を push し、`ThinkingDelta` は最後の streaming thinking に追記し、`ThinkingDone` で finished にする。
- `crates/tui/src/ui.rs` の `compute_history` は block 列を走査し、`Block::ToolCall` だけ `crate::tool::render_tool(...)` に委譲して複数 block 消費を許している。
- `crates/tui/src/tool.rs` の `Read` renderer は、連続する `Read` tool call block を `render_read_aggregate` でまとめ、1つの header と path list として表示している。
- `crates/tui/src/ui.rs` の `render_thinking` は現在、各 `ThinkingBlock` を個別に header / body preview / detail として描画している。連続 Thinking をまとめる専用 path は見当たらない。

## Unverified hypotheses

- 実装は `Read` と同様に `compute_history` 側で連続する `Block::Thinking` をまとめる renderer を導入する形が自然そう。
- 「連続」は、同じ turn 内で他の block を挟まない `Block::Thinking` の連なりとして扱うのが妥当そう。
- Normal mode では 1 header + 最新または先頭 preview、Detail mode では各 Thinking body を連結または小見出し付きで読める形にすればよさそう。

## Undecided points / open questions

- blocking な未決定点はない。
- 表示文言の細部（例: `Thoughts — 3 blocks` / `Thinking... (3 blocks)` / elapsed 表示の集約方法）は実装者判断でよい。
- ただし、連続していない Thinking や turn を跨ぐ Thinking までまとめるべきではない、という invariant は維持する。

## Background

Provider / model によっては 1 turn 内で Thinking / reasoning block が複数連続して発生する。現状の Console 表示では、それぞれが個別の Thinking 表示になり、会話ログが冗長に見える可能性がある。`Read` tool は連続する `Read` call を 1つの表示グループにまとめる既存 UX があるため、Thinking でも同様の表示集約を行う。

## Requirements

- TUI Console の history rendering で、連続する `Block::Thinking` を一つの logical display group として表示する。
- 対象は Console / single-Pod chat view の表示層に限定する。
- `Read` tool の連続 aggregation と同じく、rendering 時に複数 block を消費して一つの表示にまとめられること。
- 連続していない Thinking、別 turn の Thinking、間に `AssistantText` / `ToolCall` / `User` / `System` などを挟む Thinking はまとめない。
- `Streaming` / `Finished` / `Incomplete` が混在しても、状態が誤解されない header / body 表示にする。
- Normal / Detail / Overview mode の既存意味を保つ。
- Pod history、session log、worker history、prompt context、protocol event model、reasoning persistence は変更しない。
- In-flight snapshot から復元された unfinished Thinking と live delta continuation の表示が退行しない。

## Acceptance criteria

- 1 turn 内で `Thinking` block が複数連続する場合、Console history 上では 1つの Thinking group として表示される。
- 連続 Thinking group は、少なくとも group header と読み取り可能な preview / detail body を持つ。
- Normal mode で冗長な `Thought` / `Thinking...` header が連続表示されない。
- Detail mode では、集約された各 Thinking の本文が欠落せず読める。
- Overview mode でも 1 group として扱われ、行数が不必要に増えない。
- 間に non-Thinking block がある場合や turn を跨ぐ場合は、別 group として表示される。
- Text selection / copy の既存方針が壊れない。Thinking は現在 text-like selectable block ではないため、集約後も不用意に selectable transcript text にしない。
- Read tool aggregation、ToolCall rendering、AssistantText rendering に退行がない。
- Focused TUI rendering test が追加または更新される。

## Binding decisions / invariants

- これは TUI Console の表示改善であり、reasoning / thinking の protocol、persistence、provider event semantics の変更ではない。
- Thinking content を history / context に別形式で保存し直さない。
- 未完了 Thinking を finalized assistant history と混同しない。
- `Read` aggregation と同じく render-time aggregation として扱い、source block sequence 自体は保持する。
- Turn boundary を跨いで group 化しない。
- Non-Thinking block を跨いで group 化しない。
- Dashboard / Panel の Ticket rows や web UI は対象外。

## Implementation latitude

- `compute_history` に `Block::Thinking` の consecutive aggregation path を追加し、`tool::render_tool` と同様に consumed count を返す helper を作ってよい。
- `render_thinking` を単体 renderer として残しつつ、複数 block 用の `render_thinking_aggregate` を追加してよい。
- Header 文言、elapsed 表示、複数 state 混在時の summary は実装者判断でよい。ただし streaming / incomplete を隠さない。
- Detail mode で各 block の境界を薄い separator / index / blank line で示すか、単純連結するかは実装者判断でよい。
- Existing tests に加えて、synthetic `App { blocks: [...] }` から `compute_history` output を見る focused unit test を追加するのが自然。

## Readiness

- readiness: implementation_ready
- risk_flags: [tui-rendering, reasoning-display, block-aggregation, text-selection]

## Escalation conditions

- protocol / pod / persistence 層の変更が必要だと分かった場合。
- Thinking を selectable / copyable transcript text として扱う UX 変更が必要になる場合。
- Provider-specific reasoning metadata の扱いに踏み込む必要が出た場合。
- Console 以外の Dashboard / web / protocol UI surface まで範囲が広がる場合。
- 表示集約のために broad TUI rendering rewrite が必要になる場合。

## Validation

- `cargo test -p tui` または focused `cargo test -p tui thinking` / rendering tests。
- `cargo fmt --check`。
- `git diff --check`。
- 必要に応じて `cargo check -p tui`。
- 変更範囲が TUI / package integration に広がる場合のみ `nix build .#yoi` を検討。

## Related work

- Tickets:
  - `00001KT5D44Z0` — Unify reasoning persistence with block lifecycle
  - `00001KVMT2J25` — Pod protocol: in-flight LLM response reconnect snapshot should include unfinished blocks
  - `00001KVHX0WBE` — Dashboard / Console 呼称導入と TUI モジュール境界整理
  - `00001KTKCK8W8` — TUI chat view should render Markdown tables
  - related but not duplicate: `00001KSVP63K8`
- Files:
  - `crates/tui/src/block.rs`
  - `crates/tui/src/app.rs`
  - `crates/tui/src/ui.rs`
  - `crates/tui/src/tool.rs`
