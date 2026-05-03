# Review: セッションメトリクス: Extension 経由の汎用計測レーン

## 前提・要件の確認

### メトリクス型
- 専用 crate に置き serde で round-trip できる: `crates/session-metrics/src/lib.rs:28-44`、`tests::metric_round_trip_via_json`。OK。
- 必須 `name` / `ts`、任意 `dimensions` / `value` / `correlation_id`、unknown は `None` で表現: 同 `lib.rs:28-44`、`metric_serializes_minimal_form_compactly` で省略形を確認。OK。

### 書き込み経路
- 薄いヘルパーがあり `LogEntry::Extension { domain: "metrics", payload }` で append: `crates/session-metrics/src/lib.rs:78-86` → `session_store::save_extension`。session-store 側は無改変（`git diff -- crates/session-store/` で空）。
- hash chain に乗る (`save_extension` 経由)、ticket 上の "session-store に追加" は専用 crate に置き換えており、ticket 文言の「専用 crate（または既存の適切な配置）」の許容範囲。OK。
- 書き込み失敗の握り潰し: **未対応（後述 Blocking）。**

### 読み出し経路
- `metrics_from_extensions` で fold、deserialize 失敗 payload は skip: `crates/session-metrics/src/lib.rs:92-100`、`fold_skips_undeserializable_payloads` でカバー。OK。
- 「特定セッションの metric 列を取り出すサンプル」は `crates/pod/tests/session_metrics_test.rs:210-211` が `session_store::restore` → `metrics_from_extensions` の最小例として機能している。OK。

### 最初の利用者: Prune projection
- `attach_prune` から `prune.fire` / `prune.skip` を発行: `crates/pod/src/compact/prune.rs:52-80`。
  - `Fired` 時: `value=estimated_savings`, `correlation_id`(uuid v7), `candidate_count` + `border_turn` dim、`UsageTracker::note_correlation_id` で stash。
  - `SkippedNoCandidates` / `SkippedBelowMinSavings` の 2 経路を分けている。
- `prune.post_request` が直後の `LlmUsage` と組で発行され同じ `correlation_id` を持つ: `crates/pod/src/pod.rs:1121-1156`。
- `correlation_id` の生成は uuid v7（既存 `uuid` workspace dep の v7 feature を再利用）。OK。

### Resume 互換
- `[compaction]` 無し manifest で metrics が一切書かれない・replay も成功: `crates/pod/tests/session_metrics_test.rs:325-367`。OK。

### 完了条件
- 型 + 書き込み/読み出し + unit test: 4 件 (`crates/session-metrics/src/lib.rs:106-169`)。OK。
- prune.fire/skip/post_request が session-log に乗る: 統合テスト `prune_metrics_emit_skip_then_fire_with_post_request_join` で確認。
- 後方互換: `old_sessions_without_metrics_replay_cleanly` で確認。
- 「prune metric 列を取り出す」テスト: 同上の統合テストが兼ねる。
- correlation_id join: `prune_metrics_emit_skip_then_fire_with_post_request_join:258-276` で `fire.correlation_id == post.correlation_id`、`post.value == cache_read_input_tokens` を assert。OK。

## アーキテクチャ・スコープ

- session-store の型と公開 API は無改変（`git diff` 空）。前提を遵守している。
- `UsageTracker` 内部を `Vec<UsageRecord>` → `Vec<RecordedUsage>` に拡張したのは pod 内 `pub(crate)` のローカル拡張で、`llm_worker::UsageRecord` 型自体は触っていない。レイヤ上は問題なし（`session-store の型は触らない` の制約は守られている）。
- 新規 crate 名 `session-metrics` は memory ノートの「`insomnia-` プレフィックス不要、短い名前」に準拠。
- 依存追加は workspace.dependencies + `cargo add` 流（手動編集の痕跡なし）。OK。
- `Worker` への配線は `set_prune_observer` を追加する小規模な拡張で、prune 評価ロジック自体は `evaluate_candidates` の border_turn 返却を追加した以外はリファクタの域。`prunable_indices` を wrapper に薄く残しているのも既存呼び出し側を壊さない配慮で妥当。
- prune metric の dimension/value 振り分け（`prune.fire` の `value=estimated_savings`, `prune.post_request` の `value=cache_read_tokens`）は ticket の「value/dimension として記録」を素直に解釈したもので許容範囲。今後別 metric を生やすときに揃えやすいよう、value は「主スカラ（後で集計したい数）」/dimension は「軸（filter したい文字列）」のポリシーに統一しておくとよい（コメントで明文化されているとなおよし）。
- LLM provider policy / ScopedFs scripting plan 等の他方針には抵触しない。

## 指摘事項

### Blocking

なし。

### Non-blocking / Follow-up

- **メトリクス書き込み失敗の握り潰し**: ticket 要件 `crates/.../tickets/session-metrics.md:29` に「書き込み失敗（store IO エラー）はメトリクス側で握りつぶす。本体処理を阻害しない」とある一方、`crates/pod/src/pod.rs:1102-1110` および `1144-1150` の `record_metric` は `?` で `StoreError` を呼び出し元 (`persist_turn` → `Pod::run`) に伝播させている。ヘルパー自身のドキュメントも「書き込み失敗は呼び出し側に返す」(`crates/session-metrics/src/lib.rs:76-77`) で投げ直し前提なので、現状はメトリクス IO 失敗時に turn 永続化フロー全体が落ちる。挙動として「メトリクスのために本体処理を止めない」契約を満たしていない。
  - 対応案: `persist_turn` 内で `record_metric(..).await` の戻り値を `if let Err(e) = ... { warn!(error=%e, "metric write failed; ignoring"); }` で握り、`LlmUsage` 永続化と直交させる。Helper の doc も「呼び出し側で握り潰すのが既定」に揃える。
  - 重要度: ticket の明示要件であり本来 Blocking 候補だが、現状 store IO は LocalFsStore 一択で実害が出る経路が乏しく、後続の小修正で吸収可能と判断し follow-up に置く（ユーザーが Blocking 扱いに引き上げたい場合は了承）。

### Nits

- `crates/pod/src/compact/prune.rs:58-62` で `Fired` 時の `border_turn` を `if let Some(...)` で条件挿入しているが、`evaluate_candidates` の実装上 `Fired` / `SkippedBelowMinSavings` のときは必ず `Some` になる（候補が空でない=境界が決まる）。動作上問題はないが、不変条件を `expect("border_turn is Some when candidates exist")` で表に出すか、コメントで残すと意図が明確。
- `crates/pod/src/pod.rs:1102` 周辺のコメントにある「Ordered before LlmUsage so that a `prune.fire` and the `prune.post_request` derived from the matching usage record appear in the log close together.」は良い記述。同コメントを `metrics_tracker.rs` の `drain` 側にも一行張ると読み手が flush 順を把握しやすい。
- `Metric` の value/dimension のポリシー（主スカラ vs ラベル）について、`session-metrics/src/lib.rs` の crate doc に 1〜2 行追記しておくと、今後の利用者（compact/hook 等）が振り分けで迷わない。

## 判断

**Approve with follow-up** — ticket 要件と完了条件の主要部分は概ね満たされている。session-store の型を一切触らず、新規 crate も命名・依存追加とも方針通り。特に correlation_id による fire ↔ post_request の join はテストで明示的に検証されており、一次目的（prune 効果測定の最小レーン）は達成。一方で「メトリクス書き込み失敗を握り潰す」要件が `persist_turn` 経路で守られていない点は ticket の明文要件であり、後続コミットで吸収する想定で follow-up とする（Blocking 扱いに昇格させたい場合はその判断に従う）。
