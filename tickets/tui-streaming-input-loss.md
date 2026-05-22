# TUI: streaming 中のキー入力取りこぼしをなくす

## 背景

AI 応答中に、次の入力を先に input area へ書いておこうとすると、キー入力が無視されるタイミングがある。特に assistant text / tool block / status 更新など Pod event が高頻度に流れている間に発生しやすい。

Run 中の `Enter` 送信キューイングは `tickets/tui-input-queue.md` の範囲だが、本件はそれ以前の問題で、Run 中でも input buffer への文字入力・編集操作は常に受け付けられるべきである。

## 原因

`crates/tui/src/main.rs::run_loop` の `tokio::select!` で、Pod event 受信と terminal event 読み取りを競合させていることが原因。

terminal 側の branch は `tokio::task::spawn_blocking` で `crossterm::event::poll/read` を実行している。`spawn_blocking` の `JoinHandle` は drop しても blocking task 自体を cancel できない。そのため、Pod event 側の future が先に ready になると select はそちらを処理し、terminal branch の `JoinHandle` は捨てられるが、裏で残った blocking task はその後も `event::poll/read` を続行する。

この捨てられた blocking task がキー入力を `event::read()` で消費すると、その `TermEvent` はどこにも渡されず破棄される。Pod event が高頻度に流れる streaming / block 更新中ほどこの race が起きやすく、キー入力が「遅延」ではなく完全に input buffer に反映されない区間が発生する。

現在の frame 先頭の `try_next_event()` drain 量だけの問題ではない。根本は、cancel 不能な blocking terminal read を select 内で毎回生成し、負けた branch の結果を捨てていること。

## 要件

- terminal event の読み取りを、負けた `tokio::select!` branch で破棄される `spawn_blocking(event::poll/read)` に置かない。
  - 例: terminal reader 専用 thread / task が mpsc channel に `TermEvent` を送る形にし、select は channel 受信だけを扱う。
  - あるいは event loop を同期寄りにし、cancel 不能な read の結果を必ず次 frame で消費できる場所に保持する。
- Pod event stream が高頻度に流れている間でも、terminal key events を starvation させない。
- Run 中でも、input buffer への文字入力・編集操作は受け付ける。
  - `Enter` の送信キューイングは別 ticket の範囲なので、本件では文字入力・カーソル移動・編集を優先する。
- terminal event と Pod event の処理順序 / drain 量 / redraw 頻度を見直す。
  - 例: Pod event drain batch を小さくする、terminal event をより優先する、event loop を channel 化する、など。実装時に最小変更を選ぶ。
- streaming 中の高頻度 Pod event によって key input が失われない regression test を追加する。
  - terminal reader を channel 化する場合は、Pod event 側が select で勝ち続けても、terminal event が channel 内に残り後続 iteration で処理されることを test する。
  - 実端末依存の E2E は範囲外。
- 既存の scroll / completion / task pane key handling を壊さない。

## 完了条件

- AI 応答中 / block 更新中に文字入力しても input area に反映される。
- 高頻度 Pod event 処理中でも key event が優先的または公平に処理される。
- `cargo fmt --check`
- `cargo check --workspace`
- `cargo test -p tui`

## 範囲外

- Run 中に Enter した入力を次 turn として queue する機能（`tui-input-queue`）。
- TUI の完全な非同期 event loop 再設計。
- 実端末 E2E テスト。
