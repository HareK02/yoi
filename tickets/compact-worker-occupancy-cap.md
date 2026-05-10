# Compact worker のサーキットブレーカーを占有量ベースに統一

## 背景

Compact worker のサーキットブレーカー (`crates/pod/src/compact/worker.rs:253-269` の `CompactWorkerInterceptor`) は `compact_worker_max_input_tokens` を `UsageEvent.input_tokens` の **累積和** で見ている。一方、`UsageEvent.input_tokens` の定義 (`crates/llm-worker/src/llm_client/event.rs:76-94`) は「送信した prompt prefix の総トークン数（占有量、キャッシュ込み）」であり、Anthropic 側でも `cache_read + cache_creation` を加算してこの規約に揃えている。

結果として現行の累積メトリックは、毎ターン同じ prefix をフルカウントする「context size × ターン数」相当の値を測っており:

- compact worker は `build_summary_prompt` (`crates/pod/src/pod.rs:2630-2657`) で reasoning / ToolCall.arguments / ToolResult.content を strip した skeleton + 探索ツールという設計なので、初回 input は元 history より大幅に小さい（数十 K 程度）。
- それでも cache hit 含む prefix を毎ターン丸ごと足していくため、20-30K の skeleton を入力にツールを 2-3 回叩いた時点で 50K デフォルトに到達する。
- prompt cache が 99% ヒットしていても累積値は同じだけ増えるので、コストの近似にも安全マージンの近似にもなっていない。

メイン Worker 側の対応するしきい値 (`compact_threshold`, `compact_request_threshold`) は `Pod::total_tokens()` (`crates/pod/src/pod.rs:146-149` → `llm_worker::token_counter::total_tokens(history, &usage_records)`) を見ており、これは `UsageRecord` 列を最新測定 + バイト按分で射影した「現在の占有量」(単一の値, 累積ではない)。Compact worker でもこの正規のカウンタに統一すべき。

サーキットブレーカーとして測るべき軸は二つあり、占有量カウンタは前者だけを担当する:

1. **占有量** (cost / window 圧迫の相当値): `total_tokens()` を流用。
2. **ループ深さ** (短い context でツールを延々叩く暴走): `Worker::set_max_turns` (`crates/llm-worker/src/worker.rs:1369`) で別途上限を入れる。

## 方針

`CompactWorkerInterceptor` を、メインと同じ `UsageTracker` + `total_tokens` 機構に乗せ替える。累積メトリックは廃止する。ループ深さ対策として `compact_worker_max_turns` を新設し、`set_max_turns` 経由で compact worker に伝える。

## 要件

- `CompactWorkerInterceptor` を削除または書き換え、`pre_llm_request` の判定を `llm_worker::token_counter::total_tokens(worker.history(), &records).tokens > max_input_tokens` に切り替える。`input_so_far: AtomicU64` の累積パスは廃止。
- Compact worker にも `UsageTracker` を持たせ、`pre_llm_request` で `note_request(history.len())`、`on_usage` で `record_usage` する。メイン Pod (`pod.rs:777-780`) と同じ配線パターン。
- `compact_worker_max_input_tokens` の意味を「compact worker 側の現在占有量しきい値」に変更し、ドキュメントとデフォルト値を更新する。デフォルトは `compact_threshold` と単位が揃うため、現行 50K のままだと typical な main 側設定 (80K) に対して小さく compact 自身がそれを下回るのは妥当な範囲。実値は新セマンティクスで再評価する（要件としては「累積値ではなく占有量を測る」ことのみで固定）。
- `CompactionConfig` に `compact_worker_max_turns: Option<u32>` を追加し、`compact()` (`pod.rs:1458-`) で `summary_worker.set_max_turns` に渡す。`None` のときは無制限（既存動作）。デフォルトは要検討（仮: `Some(20)`）。
- 後方互換 shim は入れない。`compact_worker_max_input_tokens` はフィールド名を維持しつつセマンティクスだけ差し替えるため、旧設定値はそのまま新セマンティクスで解釈される。閾値のオーダーは大きくは変わらないので運用上の破壊的影響は小さい。

## 完了条件

- 通常の compact 実行で、cache hit 込みの prefix がフルカウントされなくなり、`build_summary_prompt` の skeleton + 数回のファイル読み程度では cap に当たらない。
- 短い context でツールを延々呼び続ける疑似ケースで、`compact_worker_max_turns` により compact run が打ち切られる。
- `Pod::total_tokens()` と compact worker の占有量推定で同じ算出経路 (`llm_worker::token_counter::total_tokens`) が使われている。

## 範囲外

- `compact_threshold` / `compact_request_threshold` 自体のセマンティクス・既定値変更。
- Compact worker が更に compact をかける（meta-compact）。
- `compact_auto_read_budget` ロジックの変更。
- token 推定アルゴリズム自体の改善。

## 影響範囲

- `crates/pod/src/compact/worker.rs`: `CompactWorkerInterceptor` の実装、`input_so_far` 経路の削除。
- `crates/pod/src/pod.rs`: `compact()` の compact worker 構築箇所 (`UsageTracker` 配線、`on_usage` 差し替え、`set_max_turns` 呼び出し)。
- `crates/pod/src/compact/usage_tracker.rs`: 既存 `UsageTracker` を compact worker からも使うため、必要なら可視性 / API の調整。
- `crates/manifest/src/lib.rs`, `crates/manifest/src/config.rs`, `crates/manifest/src/defaults.rs`: `compact_worker_max_input_tokens` のドキュメント更新、`compact_worker_max_turns` の追加とカスケード。
- `crates/pod/src/compact/worker.rs` のユニットテスト、および compact 関連の統合テスト。

## Review
- 状態: Approve with follow-up
- レビュー詳細: [./compact-worker-occupancy-cap.review.md](./compact-worker-occupancy-cap.review.md)
- 日付: 2026-05-11
