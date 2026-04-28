# Review: Thinking ブロックの TUI 表示

## 前提・要件の確認

### Worker API
- `Worker::on_thinking_block(setup)` は `crates/llm-worker/src/worker.rs:245-253` に追加、`on_text_block` (231-238) と完全に対称な setup シグネチャ。`block.on_delta` / `block.on_stop` は `ThinkingBlockScope` (`callback.rs:108-133`) で提供。`Send + Sync + 'static` 制約も同等。
- 内部実装 `ClosureThinkingBlockHandler` は `Handler<ThinkingBlockKind>` を実装し (`callback.rs:146-171`)、`TextBlockEvent` のハンドラ (74-96) と一語一語まで同形。バッファ蓄積 → Stop で `on_stop(&buffer)` という流れで、本文を出さない provider のときも `on_stop("")` が呼ばれる。

### Protocol
- 3 イベント追加: `Event::ThinkingStart` / `ThinkingDelta { text }` / `ThinkingDone { text }` (`crates/protocol/src/lib.rs:181-195`)。doc コメントに「Delta 0 件のまま Start → Done が来る場合がある」「複数 thinking block を許容」を明示。
- `event_thinking_roundtrip` (`lib.rs:487-512`) で 3 variant の serde 往復と `event` フィールドの snake_case (`thinking_start`) を検証。要件の「TextDone と同じ流儀で text に完成形を載せる」も実装と整合。

### Pod Controller
- `worker.on_thinking_block` で 3 イベントへブリッジ (`crates/pod/src/controller.rs:144-162`)。setup 内で `tx.send(Event::ThinkingStart)` を無条件に発火し、その後 `block.on_delta` / `block.on_stop` を登録するという順序。setup 自体が Start ハンドリング時に呼ばれるので「ブロックごとに 1 度だけ」という意図と一致。コメント (146-148) で意図を残しているのも好印象。

### TUI
- `Block::Thinking(ThinkingBlock)` を `crates/tui/src/block.rs:25` に追加。`ThinkingBlock { text, state }` と `ThinkingState::{Streaming{started_at}, Finished{elapsed_secs}, Incomplete{elapsed_secs}}` (`block.rs:40-57`) で要件 3 状態を表現。`started_at: Instant` は履歴復元には不要なので enum variant に持たせる選択は妥当。
- イベントハンドラ (`crates/tui/src/app.rs:118-147`):
  - `ThinkingStart`: `assistant_streaming = false` で text streaming を切ってから push (アシスタント本文の途中に thinking が割り込んでも以降の `TextDelta` は新規 `AssistantText` ブロックになる)。
  - `ThinkingDelta`: `last_streaming_thinking_mut` で直近の Streaming に append。`TurnHeader` でストップして前 turn を漁らない実装は意図通り。
  - `ThinkingDone`: 空テキストで来た場合は累積バッファを優先（Anthropic は Done が full text 持ちで、Delta も流れる二重ケース対策として `if b.text.is_empty()` 分岐を入れている）。`elapsed = started_at.elapsed()` を Streaming 時のみ採取し、それ以外（History 由来）は `None` で残す。
- `TurnEnd` 時 `mark_orphan_thinking_incomplete` を呼ぶ (`app.rs:151`、定義 327-341)。`Streaming` を `Incomplete{elapsed_secs: Some(...)}` に転落させる仕様で、要件「経過時間を凍結した状態で履歴に残す」と整合。`TurnHeader` で打ち切るので前 turn 情報を破壊しない。
- History 再生 (`app.rs:467-486`): `type: "reasoning"` を `Block::Thinking { Finished{elapsed_secs: None} }` で復元。`text` が空なら `summary` 配列を改行 join、これも仕様 (`Item::Reasoning { text, summary, ... }` の両表現対応) 通り。
- レンダリング (`crates/tui/src/ui.rs:545-605`):
  - `Streaming` → `Thinking... (Xs)` ヘッダ + 末尾行プレビュー (Normal)。
  - `Finished` → `Thought for Xs` (`elapsed=None` 時は `Thought` のみ) + 冒頭行プレビュー (Normal)。
  - `Incomplete` → `Thinking interrupted (Xs)` でツールの Incomplete と同質の表現。仕様で表現は固定されていないが、視覚的に「凍結された Thinking」が分かるので妥当。
  - `Mode::Overview` はヘッダ 1 行のみ、`Mode::Detail` は本文全行。3 モードとも要件通り。
  - 1 行プレビューは `width.saturating_sub(2)` を `truncate_with_ellipsis` に渡しており、左の `"  "` インデント分を引く配慮あり。
- `MessageKind::Thinking` (Magenta + ITALIC、`ui.rs:840`, `851-853`) で他カテゴリと区別可能。
- `fmt_elapsed` (`ui.rs:628-634`) は 60s 未満 `{n}s`、それ以上 `{m}m{ss}s`。仕様の `1m23s` と一致。

### イベント欠落耐性
- `ThinkingDone` 欠落時は `mark_orphan_thinking_incomplete` で `Incomplete` に落ちる。`ThinkingDelta` のみ来て `ThinkingStart` が来ないケースは `last_streaming_thinking_mut` が `None` を返して silently drop（前ブロックを書き換えない）。`ThinkingStart` のみで `ThinkingDelta` も `ThinkingDone` も来ない provider は要件範囲内で「本文無し Streaming → TurnEnd で Incomplete 化」という動作になる。
- panic 経路は無い (unwrap / index 直接アクセス無し)。

### History 再生
- `restore_history` の reasoning ハンドリングは要件通り。`Finished{elapsed_secs: None}` で `Thought` (時間なし) として表示される。

### ライブタイマー再描画
- `crates/tui/src/main.rs:206-227` の run_loop は 50ms (`Duration::from_millis(50)`) ごとに `event::poll` を打ち、その後 `terminal.draw` を毎ループ末尾で呼ぶ (239)。つまり ~20Hz で常時再描画される構成。仕様要件「1Hz 程度で十分」を 20 倍上回る粒度で確実に満たす。新規にタイマーを足さなくて良いという判断は妥当。

## アーキテクチャ・スコープ

- **層分離**: llm-worker は handler / closure / scope を「Thinking 用に対称コピー」するだけで、provider 別の解釈や TUI 都合の分岐を一切持ち込んでいない。protocol は wire 表現の追加のみ、pod controller は単なるアダプタ、TUI 側に状態機械が閉じている。`feedback_llm_worker_scope.md`「llm-worker は低レベル基盤に留める」に則っている。
- **Event::ThinkingStart の追加 (TextDelta/TextDone との非対称性)**: 既存パターンでは AssistantText は最初の Delta で lazy 生成だが、Thinking は本文を持たない provider があるため Start イベントが必要。意図的な非対称で、`crates/protocol/src/lib.rs:181-187` の doc コメントにも「Delta が optional」と明記されている。チケット要件（本文を出さない provider でも `Thinking...` ヘッダを出す）に直結し、後付けでも除去でも済まない。設計判断として妥当。
- **`ThinkingState::Streaming { started_at: Instant }`**: `Instant` は serialize 不可だが、Streaming 状態のブロックが永続化される経路は無い (history は `Reasoning { text, summary }` で再生 → `Finished{None}` 確定) ため問題なし。enum variant に直接持たせる選択は、`Finished{elapsed_secs}` への遷移時に値が一意に確定するので素直。
- **`mark_orphan_thinking_incomplete` と `mark_orphan_tool_calls_incomplete` の構造類似**: ToolCall 側 (357-374) は「最初に Done/Error なブロックに当たったら break」する一方、Thinking 側 (327-341) は `TurnHeader` まで素通しで全 `Streaming` を Incomplete 化する。Thinking ブロックが turn 内で複数並ぶことは仕様で許容されているため、走査して全部潰すこちらの実装が正しい。微妙な差だがコメントで意図を残しておくと将来助かる（Nits）。
- **無関係な改変なし**: Cargo.toml / 依存追加なし、新規 crate 無し、別 ticket スコープへの侵食なし。範囲外として明記された 4 点（reasoning_tokens、Redacted Thinking、Markdown レンダリング、prune）には一切手を出していない。

## 指摘事項

### Blocking
なし。

### Non-blocking / Follow-up

- **`ThinkingDone` の text マージロジックが provider 仕様の取り違えに脆弱** — `app.rs:132-137` で `b.text.is_empty()` のときだけ `text` (Done payload) を差し込む。Anthropic は Delta も流しつつ Done に full text を載せるので、両方来た場合は Delta 蓄積が優先される (= Done payload を捨てる) 実装。`ClosureThinkingBlockHandler` 側でも buffer は Delta から作っているので、buffer と Done text が乖離する provider が現れた場合（例: Done が「整形済み」で Delta が「raw」など）に検出が遅れる可能性。現状の正規化レイヤ (`DeltaContent::Thinking`) はそのケースを起こさない作りなので実害なし。doc に「Delta が来ていれば Delta 優先」とコメントを 1 行残しておくと将来助かる。
- **`last_streaming_thinking_mut` が「最も新しい Streaming」を返す前提** — `app.rs:314-325` は最後尾から `Streaming` を見つけたら即返す。仕様上 thinking block は逐次的なので問題ないが、provider が並列的に thinking を流してきたら混信する。現在の Timeline 実装では同時に Streaming な thinking block が 2 つ存在しないため健全。ガード（block index でリンク）は YAGNI で良いが、protocol に「同時に Streaming は 1 件まで」を明文化するなら別チケットで。
- **`mark_orphan_thinking_incomplete` の break 条件** — ToolCall 版は「Done/Error にぶつかったら break」して走査短縮する一方、Thinking 版は `TurnHeader` まで全部見る。turn 内の Finished な thinking block を毎回 visit しても overhead は実質ゼロだが、コメントで「全 streaming を潰したいので break しない」と一言残せると意図が読み取りやすい (`crates/tui/src/app.rs:327-341`)。

### Nits
- `crates/tui/src/ui.rs:609-616` `trailing_line_preview` doc に「the live "what is it thinking now" 1-liner」とあるが、`text.rsplit_once('\n').map(|(_, tail)| tail).unwrap_or(text).trim_end()` は 末尾が改行で終わる Streaming 中の bufer に対して空文字を返す（rsplit_once は最後の改行で分割するので tail が空）。発生条件は「直前まで本文があり、最後の delta で改行のみが来た瞬間」というレアケース。preview が一瞬空になるだけで害は無い。気になるなら `rsplit('\n').find(|l| !l.is_empty())` でも可。
- `Event::ThinkingStart` 用 setup callback の中で `tx.send(Event::ThinkingStart)` を呼ぶスタイル (`controller.rs:149`) は、closure の意味的には「setup callback はブロックの metadata を受け取るためのもの」というニュアンスからは少しズレる。`on_thinking_block_start(|| ...)` のような専用フックを切る案もあるが、ToolUse 側も同様 (`controller.rs:166-169`) で start 通知を setup で打っているため一貫性は取れている。現状でよい。
- `crates/tui/src/block.rs:48-50` の `Streaming` doc コメントで「`started_at` is `None` only for blocks materialised from `Event::History`」と書かれているが、`started_at` は `Instant`（`Option<Instant>` ではない）で、History 由来は `Finished{None}` として作られる。コメントが古い設計を引きずっている。読み手を惑わすので `started_at: Instant` の意味だけにトリムすると親切。

## 完了条件のマッピング

- Anthropic の extended thinking で「`Thinking... (Xs)` + 本文 1 行」が live: `ThinkingStart` + `ThinkingDelta` で `ThinkingState::Streaming` ブロックに append、`render_thinking` Normal 分岐 (`ui.rs:584-602`) で末尾行プレビュー、`run_loop` の 50ms 再描画でタイマー更新。OK。
- 終了後 `Thought for Xs` 残存: `ThinkingDone` ハンドラで `Finished{elapsed_secs: Some(...)}`、ヘッダ表現 (`ui.rs:554-557`)。OK。
- 本文を流さない provider でもヘッダが立つ: pod controller の setup 内 unconditional `ThinkingStart` 発火 (`controller.rs:149`)。OK。
- `detail` モードで本文全行展開: `ui.rs:576-583`。OK。
- 同一 turn 複数 thinking block: ハンドラは「直近の Streaming」しか触らないので、Done → Start で次のブロックを push する流れで独立して扱われる。OK。
- History 再生で `Finished{None}`: `app.rs:467-486`。OK。
- `ThinkingDone` 欠落で panic せず Incomplete 表示: `mark_orphan_thinking_incomplete` + Incomplete レンダリング (`ui.rs:558-561`)。OK。
- 既存表示の非回帰: cargo build / test workspace pass を信頼。Event::ThinkingStart の追加が `Event` 列挙の網羅マッチに影響しないか念のため確認 → app.rs / protocol テスト以外で `match Event` する場所は影響なし（ハンドラはすべて閉じた match を更新済み）。

## 判断

**Approve** — チケットの方針 / 要件 / 完了条件はいずれも実装に直接対応しており、範囲外として宣言した 4 項目にも踏み込んでいない。`Event::ThinkingStart` の新設は「本文を出さない provider でもヘッダを立てる」という機能要件のために必要で、TextDelta との非対称性は意図的であり doc にも明記されている。アーキテクチャ上の歪みなし、Blocking 指摘事項なし。Non-blocking はいずれもコメント補強や将来検討で、実装そのままで完了させて良い。
