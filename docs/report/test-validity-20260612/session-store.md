# テスト妥当性レビュー: session-store
- 評価: 概ね良い

## 確認範囲

- `crates/session-store/README.md`
- `crates/session-store/Cargo.toml`
- `crates/session-store/src/`
  - `lib.rs`
  - `store.rs`
  - `fs_store.rs`
  - `segment.rs`
  - `segment_log.rs`
  - `logged_item.rs`
  - `system_item.rs`
  - `event_trace.rs`
- `crates/session-store/tests/`
  - `fs_store_test.rs`
  - `session_test.rs`
  - `common/mod.rs`
- 実行確認: `cargo test -p session-store`

## 現在のテストがよくカバーしていること

`session-store` の主要責務である「append-only JSONL session log を読み書きし、Worker 復元用 state に replay する」部分は、短時間レビューとしては十分に押さえられています。

良い点:

- `collect_state` の基本 replay が広くテストされている。
  - 空の log
  - `SegmentStart`
  - user / assistant / tool call / tool result
  - `TurnEnd`
  - `ConfigChanged`
  - `LlmUsage`
  - `Invoke` marker が state を変えないこと
  - `Extension`
  - typed `protocol::Segment` の保存と flattened user message への復元
- `LoggedItem` の stable mirror が比較的よく検証されている。
  - message
  - tool call
  - tool result content / error flag
  - reasoning encrypted content
  - reasoning signature
  - legacy signature 欠落 JSON
  - serde tag shape
- `SystemItem` は、LLM に入る `history_text` が stored body 由来であること、reminder wrapping、legacy default source、JSON round-trip が確認されている。
- `FsStore` は tempfile を使った実 FS の読み書きで確認されている。
  - append / read round-trip
  - `create_segment`
  - `list_sessions`
  - `list_segments`
  - `exists`
  - missing segment
  - trace file が segment log と分離されること
  - `read_entry_count`
  - `lookup_session_of`
- `session_test.rs` は mock LLM + actual `llm_worker::Worker` を通して、単純応答・tool call・pause・restore・fork 系を実行している。単なる pure unit だけでなく、crate 境界上の使われ方もある程度守れている。
- fork lineage について、fresh-session fork / in-session `fork_at` / live conflict `ensure_head_or_fork` / nested past fork まで触れているのは良いです。特に「source segment を mutate しない」方針をテストで固定できています。

## 不足・疑問のあるテスト

現状は「基本機能の回帰検出」としては強い一方で、session-store の重要 invariant に対して未検証の穴がいくつか残っています。

- `create_compacted_segment` が未テスト。
  - この crate の説明上、compaction は fork と並ぶ segment lineage 操作です。
  - `compacted_from`、同一 `session_id` 継承、seed history/config/system prompt の保存が直接検証されていません。

- `Store::truncate` が未テスト。
  - trait default 実装で `read_all -> truncate -> create_segment` するため、empty-turn rollback などの重要用途で壊れてもこの crate 単体では検出しにくいです。
  - `entries_len > len` の `StoreError::Corrupt` も未検証です。

- corrupt JSONL の扱いが未テスト。
  - `FsStore::parse_jsonl` は blank line を無視し、serde failure を `StoreError::Corrupt { line, message }` に変換します。
  - line number、blank line skip、部分的に壊れた file で read が fail closed することは永続ログ store として重要ですが未確認です。

- `FsStore` の filtering / layout invariant が一部弱い。
  - top-level old flat files を `list_sessions` が無視すること。
  - invalid UUID directory を無視すること。
  - `.trace.jsonl` を `list_segments` が segment として拾わないこと。
  - invalid segment filename を無視すること。
  - `list_segments` の newest-first order は `contains` 中心で、順序を強く確認していません。

- `save_delta` / `classify_history_item` の直接テストが薄い。
  - user message を skip する contract は integration helper 経由では使われていますが、直接の単体テストがありません。
  - reasoning が `AssistantItem` になること、tool result error/content が `ToolResult` として保存されること、unknown/future-ish branch の defensive behavior は十分固定されていません。

- `RunErrored` path が未テスト。
  - `run_and_persist` helper には error branch がありますが、最後に `result.unwrap()` するため、実際には error persistence の検証に使えません。
  - `save_run_errored` と replay 時の `last_run_interrupted` 反映は、失敗 turn の session 復元に関わるため追加価値が高いです。

- `SystemItem` の replay path が直接弱い。
  - `SystemItem::history_text` 自体はテストされていますが、`LogEntry::SystemItem` を `collect_state` したときに `Item::system_message` へ入ること、`append_system_item` 経由で store に保存されることは明示的にテストされていません。

- fork 系の一部 assertion が弱い。
  - `session_fork_at_truncates_within_session` は fork state と source-at-fork の `history.len()` だけを比べています。content / item kind / config / system prompt / lineage まで比較した方が妥当です。
  - `at_turn_index == 0` を、実際に turn 後の source segment に対して使うケースが薄いです。
  - 存在しない `at_turn_index` の扱いは未固定です。現実装は `unwrap_or(entries.len())` で末尾 fork になりますが、それが意図かどうかをテストで明確にした方がよいです。
  - `ensure_head_or_fork` の no-op branch（count equal）と、store count が writer tally より小さい場合の policy が未検証です。

- integration tests の一部は smoke test 寄りで、壊れ方を見逃しやすい。
  - `session_restore_round_trip` は history の長さだけで、復元 item の content / role / tool call id までは比較していません。
  - `session_run_with_tool_call` は `ToolResult` / `AssistantItem` の存在だけを確認しており、順序・call id・tool result content・final assistant text を見ていません。
  - `session_run_logs_entries` はコメントでは最低 5 entry と書きつつ `>= 4` で、`UserInput` の存在も明示確認していません。

- real concurrency / append atomicity は未テスト。
  - この crate は append-mode JSONL を concurrency 境界として扱っていますが、複数 clone / thread からの append が line corruption しないことは未確認です。
  - ただし OS/filesystem 依存が強いので、軽い regression test に留めるのが妥当です。

## 追加を提案するテスト

優先度順に追加するなら以下です。

1. `create_compacted_segment` の lineage test
   - source session を作る
   - compacted segment を作る
   - first entry が `SegmentStart { session_id: source_sid, compacted_from: Some(...) }`
   - `forked_from` は `None`
   - history/config/system prompt が seed state から復元される

2. `Store::truncate` default behavior test
   - 3〜4 entries を作る
   - 2 entries に truncate
   - `read_all` が prefix のみ返す
   - `read_entry_count` が一致する
   - len 超過 truncate が `StoreError::Corrupt` になる

3. corrupt JSONL test
   - tempfile 上に直接 invalid JSON line を置く
   - `read_all` が `StoreError::Corrupt { line, .. }` を返す
   - blank line が count / parse に影響しないことも併せて確認する

4. `FsStore` layout filtering test
   - root 直下の `<uuid>.jsonl` を無視
   - invalid directory を無視
   - session dir 内の `<segment>.trace.jsonl` と invalid filename を `list_segments` が無視
   - segment order を newest-first で assertion

5. `save_delta` direct unit/integration test
   - input に user message / assistant message / reasoning / tool call / tool result を混ぜる
   - user message が保存されないこと
   - classification と order が期待通りなこと

6. `RunErrored` persistence test
   - `save_run_errored` を直接使うだけでもよい
   - 可能なら mock client error を発生させ、Pod 相当の persistence sequence で `RunErrored` が書かれることを確認する
   - `collect_state` / `restore` で `last_run_interrupted` が反映されることを確認する

7. `SystemItem` replay/store test
   - `append_system_item` で notification / task reminder などを保存
   - `restore` 後の `history` に `role: system` の message と stored body が入ることを確認する

8. fork tests の assertion 強化
   - `history.len()` だけでなく、復元 content / role / item kind を比較する
   - `forked_from` の `segment_id` と `at_turn_index` を `fork_at` test でも確認する
   - `at_turn_index == 0` on non-empty source と nonexistent turn index の policy を固定する

## 実行したコマンド

```sh
cd /home/hare/Projects/yoi && cargo test -p session-store
```

結果:

- unit tests: 29 passed
- `tests/fs_store_test.rs`: 8 passed
- `tests/session_test.rs`: 9 passed
- doctest: 1 ignored
- overall: passed

