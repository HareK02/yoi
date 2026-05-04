# TUI: Compaction 中の進行表示

## 背景

Pod で compact が走ると `Event::CompactStart` が 1 本流れ、その後 compact worker (要約 / auto-read 判定 / リファレンス選定) が走り終えるまで TUI には何も流れない。完了時に `Event::CompactDone` または `CompactFailed` が来てようやく次の block が積まれる。

実時間で数十秒〜分単位かかる区間が完全な無音になり、ユーザーからは「固まった」「Pod が落ちた」と区別がつかない。

現状の TUI 表示 (`crates/tui/src/app.rs:651-661`、`crates/tui/src/ui.rs:799-821`) は `CompactStart` / `CompactDone` / `CompactFailed` をそれぞれ独立した append-only な block として積んでいるだけで、進行中であることや経過時間は出ていない。

ThinkingBlock は同種の「LLM が応答するまでの待ち時間」を `ThinkingState::Streaming { started_at }` + ライブで `Thinking... (Xs)` 表示で扱っている (`crates/tui/src/block.rs:67-78`)。Compact も同じパターンに揃えたい。

## 要件

- compact 中であることがライブで分かる。経過秒数を 1 行で表示する (`Compacting... (Xs)` 程度)
- 完了 / 失敗時は進行表示が結果行に **置き換わる**。1 回の compact で block が 2 個積まれるのではなく、Start で積んだものを Done / Failed が更新する
- 完了行は新 session_id の short prefix と所要時間を含む
- 失敗行はエラー文と所要時間を含む
- 進行中のまま `Event::Shutdown` 等で取り残された場合は ThinkingState::Incomplete に相当する終端状態に落とす
- ステータスライン (`draw_status`) はこのチケットでは触らない。block 側のライブ表示のみ

## 完了条件

- compact が走っている間、TUI 末尾に `Compacting... (Xs)` が出て、s が秒単位で進む
- compact 成功で同じ行が `[compact] done (new session XXXXXXXX) (Ys)` 相当に更新される
- compact 失敗で同じ行が `[compact error] ... (Ys)` 相当に更新される
- 1 回の compact イベント列で `Block::Compact` は 1 個しか積まれない (履歴を遡っても重複しない)
- 既存テストが通る

## 範囲外

- 要約 worker の中身 (read_file ループ等) を TUI に流す仕組み
- ステータスラインへの `Compacting…` 表示
- session_id の TUI 表示全般 (旧 / 新 / 親 / 子セッション系譜の可視化は別チケット)
- compact がそもそも動いているかどうかの設定確認 UI

## 関連

- `docs/compaction.md` — compact のトリガーとイベント並びの全体像
- `crates/tui/src/block.rs` の `ThinkingBlock` / `ThinkingState` — 同型のライブ進行表示の前例
- `crates/tui/src/app.rs:714-746` — `last_streaming_thinking_mut` / `mark_orphan_thinking_incomplete` の参考実装
