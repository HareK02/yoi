# llm-worker: history append を callback 経由の単一経路に閉じる

## 背景

`pod-interrupt-prep-internalize` レビュー過程で、`Worker` の history append API に **callback を踏む経路** と **踏まない経路** が併存していることが顕在化した。

- callback を踏む経路（worker 内部 private）: `extend_history_with_callbacks` — `emit_history_append(&item)` を呼んでから `self.history.push(item)`。`Worker::run` 内部の streaming commit / tool result commit はこの経路。
- callback を**踏まない**経路（外部公開）: `Worker::push_item` / `Worker::extend_history` / builder の `with_items` — `self.history.push` / `extend` するだけで `emit_history_append` を呼ばない。

session-log への永続化は `Pod::wire_history_persistence` が `Worker::on_history_append` を立て、callback 内で `classify_history_item` → `LogEntry::AssistantItem` / `LogEntry::ToolResult` として `writer.append_entry` する作りになっている。

結果として「callback 不発火経路で append すると in-memory `worker.history` には載るが session-log の独立エントリにはならない」という非対称が API 契約として残っている。実際 production では `Pod::apply_interrupt_prep` (`crates/pod/src/pod.rs:1438` / `:1441`) がこの不発火経路を踏んでおり、Paused→Run 時の orphan tool_result closure と interrupt system note が session-log に独立行として残らない。

CLAUDE.md の「LLM コンテキスト加工原則」は「新しい input を context に乗せたいなら、必ず先に `worker.history` に append して commit すること。`history.json` への永続化はそこから自動的についてくる」と謳っているが、この「自動的についてくる」が現状の API 契約では保証されていない。**契約として callback バイパスが可能な設計であってはならない**。

## 要件

- `Worker` の history を成長させる経路を、必ず `on_history_append` callback を踏む単一経路に統一する。
- 外部から呼び出せる「callback 不発火な history append」API を廃止する。
  - 対象: `Worker::push_item`、`Worker::extend_history`、`WorkerBuilder::with_items` 系
  - 内部実装の `extend_history_with_callbacks` 相当を唯一の append プリミティブに格上げする（命名は実装時に整理）
- `Pod::apply_interrupt_prep` を新 API に乗せ替える。乗せ替えの副作用として、Paused→Run 時の orphan tool_result closure / interrupt system note が `LogEntry::ToolResult` / `LogEntry::SystemItem` 系として session-log に独立エントリで記録されるようになる（これは本チケットで肯定的に取り込む変化）。
  - 注: system message は `PodInterceptor` 経由で `LogEntry::SystemItem` として典型化されている経路があるので、callback 内のフィルタ (`pod.rs:381-389` の `Role::System` skip) との重複を作らないこと。interrupt system note は callback 単独で書く / interceptor 単独で書く / 両方書かない、のいずれかに整合させる。実装時に確認。
- `worker_state_test.rs` を含む既存テストは、新 API に書き換えるか、builder 段階で history を仕込む正当な用途として整理する。前者を基本とする。

## スコープ外

- `Worker::clear_history` の扱い（append ではなく削除なので別論点）。
- `LogEntry` バリアントの設計変更。
- callback で書かれるエントリの形式・命名の整理（`classify_history_item` の現行挙動を前提として進める）。
- `Notify` / `PodEvent` / `<system-reminder>` 系の history 反映ポリシー全体（`system-reminder` 注入機構の汎用化として別途整理する。本チケットはあくまで「API 契約から callback バイパスを消す」レイヤに閉じる）。

## 完了条件

- `Worker::push_item` / `Worker::extend_history` / `WorkerBuilder::with_items` 等、callback を踏まずに `worker.history` を成長させられる public API がコードベースに存在しない。
- `cargo check --workspace` と `cargo test -p llm-worker --lib` / `cargo test -p pod` が通る。
- `apply_interrupt_prep` が新 API 経由になり、interrupt 前処理由来の append が session-log の独立エントリとして残ることを `~/.insomnia/sessions/*.jsonl` で目視確認できる（手順をレビュー時の備考に記載）。
