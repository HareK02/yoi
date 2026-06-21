---
title: 'Pod protocol: in-flight LLM response reconnect snapshot should include unfinished blocks'
state: 'queued'
created_at: '2026-06-21T10:02:01Z'
updated_at: '2026-06-21T10:56:32Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['protocol', 'session-history', 'persistence', 'tui-reconnect', 'stream-state']
queued_by: 'workspace-panel'
queued_at: '2026-06-21T10:56:32Z'
---

## User claims / request snapshot

- プロトコル実装の問題として、LLM 応答中に接続すると、まだ完了していない block の途中内容が欠落する。
- 応答完了後に接続し直すと見える。
- 対象 workspace は `yoi`。Panel handoff の orchestrator Pod は `yoi-orchestrator`。

## Confirmed facts / sources

- 既存 Ticket 確認:
  - active duplicate は見当たらない。
  - `00001KSVP63K8` は in-flight TUI composer injection で、実行中 turn への入力注入の設計 Ticket。今回の「途中出力の late attach / reconnect 表示欠落」とは別件。
- `crates/protocol/src/lib.rs`
  - `Event::Snapshot` は接続開始時に一度送られ、`entries` は subscribe 時点の session-log mirror。
  - コメント上、Snapshot 後の live 更新は `TextDelta` / `ToolCall*` / `ToolResult` 等で流れ、generic な committed entry broadcast はない。
- `crates/pod/src/segment_log_sink.rs`
  - `SegmentLogSink::subscribe_with_snapshot()` は committed `LogEntry` の prefix と live receiver を gap-free に分ける設計。
  - `AssistantItem` / `ToolResult` などは mirror には反映されるが live broadcast されず、live 表示は streaming events に依存する。
- `crates/pod/src/controller.rs`
  - text / thinking / tool-call args の途中 delta は `Event::TextDelta` / `ThinkingDelta` / `ToolCallArgsDelta` として direct broadcast される。
- `crates/tui/src/app.rs`
  - Snapshot は `restore_snapshot(&entries, greeting)` で session-log entries から復元される。
  - live delta は TUI 側で block に追記される。
- 以上から、コード上も「接続前に流れたがまだ committed history になっていない途中 delta」を late subscriber が復元する lane が見当たらない。

## Unverified hypotheses

- 実際の欠落原因は、未完了 block の accumulator が `Event::Snapshot` に含まれず、live subscriber は subscribe 後の delta しか受け取れないことだと思われる。
- 応答完了後に再接続すると見えるのは、finalized assistant/tool history が session log mirror に committed され、Snapshot entries から復元できるためだと思われる。
- 修正は、protocol-level に in-flight block state を snapshot へ含める、または bounded replay/sequence 付き live event buffer を導入する形が自然そう。

## Undecided points / open questions

- blocking な未決定点はなし。
- 実装戦術として、`Event::Snapshot` に structured `in_flight` state を追加するか、sequence 付き replay buffer を使うかは Coder がコード調査して選んでよい。
- protocol crate の wire shape 変更なので、既存 serde roundtrip / older snapshot fallback をどこまで持つかは実装時に最小限で判断する。不要な後方互換は作らない。

## Background

LLM の応答中に Console / TUI / attach client が接続した場合、ユーザーはその時点までに出ている assistant text、thinking、tool-call args などの unfinished block を見られる必要がある。現在の構造では、接続時 Snapshot は committed session-log entries だけを seed し、途中 delta は live broadcast のみなので、接続前に流れた unfinished delta が見えない可能性がある。

## Requirements

- LLM 応答中に新しく接続・再接続した client が、接続時点までに蓄積済みの unfinished block 内容を表示できるようにする。
- 対象 block は少なくとも以下を含む:
  - assistant text streaming block
  - thinking/reasoning streaming block
  - tool-call arguments streaming block
- Snapshot と Snapshot 後の live events の境界で、欠落も重複も起こさない。
- 応答完了後の reconnect では、従来通り finalized session-log から完全な表示を復元できること。
- protocol-level の整合性として直す。TUI だけの偶然の workaround にしない。
- 未完了 model output を、finalized assistant history として誤って永続化しない。
- prompt/history/context に hidden injection しない。

## Acceptance criteria

- LLM 応答中に client が接続した場合、接続前に生成済みの unfinished text / thinking / tool-call args が表示される。
- 接続後に続く delta は同じ block に継続して追記され、途中内容の欠落・二重表示がない。
- Run 完了後に接続し直しても、finalized transcript は従来通り Snapshot entries から復元される。
- Snapshot/live 境界の gap-free / duplicate-free 性をテストで確認する。
- TUI の `Event::Snapshot` 処理と live delta 処理の regression がない。
- focused validation として、少なくとも protocol/pod/TUI の関連 test または unit test が追加・更新される。

## Binding decisions / invariants

- 「応答完了後に接続し直せば見える」は workaround であり、正しい完了条件ではない。
- Late attach は、実行中 Pod の現在表示可能な stream state を復元できるべき。
- Committed session-log の gap-free semantics は壊さない。
- Unfinished block は finalized assistant history と混同しない。
- Provider stream 自体を巻き戻したり mutate したりしない。
- Hidden context/history injection はしない。

## Implementation latitude

- `Event::Snapshot` に in-flight block state を追加する案、または bounded/sequence 付き stream replay buffer を導入する案のどちらでもよい。
- Controller / Pod 側で text/thinking/tool-call args の current accumulator を保持する設計にしてよい。
- TUI 側は Snapshot から unfinished block を seed し、その後の live delta を同一 block に継続適用できればよい。
- wire compatibility は必要最小限。長期保守・型安全性を優先する。

## Readiness

- readiness: implementation_ready
- risk_flags: [protocol, session-history, persistence, tui-reconnect, stream-state]

## Escalation conditions

- unfinished output をどの durable history item として永続化するかの設計変更が必要になった場合。
- Snapshot に含める in-flight state が大きくなり、boundedness / memory usage / truncation policy が必要になった場合。
- protocol public surface として互換方針を決める必要が出た場合。
- TUI だけではなく Dashboard / Pod list preview など複数 surface の UX 方針に広がる場合。

## Validation

- `cargo test -p protocol` の relevant roundtrip / serialization tests。
- `cargo test -p pod` の subscriber/snapshot/live-stream focused tests。
- `cargo test -p tui` または targeted app snapshot/live delta tests。
- `cargo fmt --check`
- 必要なら `cargo check -p pod -p tui -p protocol`

## Related work

- Related but not duplicate:
  - `00001KSVP63K8` — Support immediate in-flight TUI composer injection
- Relevant files:
  - `crates/protocol/src/lib.rs`
  - `crates/pod/src/segment_log_sink.rs`
  - `crates/pod/src/controller.rs`
  - `crates/pod/src/pod.rs`
  - `crates/tui/src/app.rs`
