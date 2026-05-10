# Review: Compact worker のサーキットブレーカーを占有量ベースに統一

レビュー対象: commit `ef0cdf7` (base `d818b37`).

## 前提・要件の確認

- 要件1「`CompactWorkerInterceptor` を `total_tokens` ベースに切り替え、`input_so_far: AtomicU64` 経路を廃止」: 満たされている。`crates/pod/src/compact/worker.rs:249-274` で `usage_tracker.records()` → `llm_worker::token_counter::total_tokens(context, &records)` に置き換わっており、`AtomicU64` import / フィールドも削除されている。`crates/pod/src/pod.rs:1456` 周辺の `use std::sync::atomic` も消えている。
- 要件2「Compact worker に `UsageTracker` を持たせ、`pre_llm_request` で `note_request`、`on_usage` で `record_usage`」: 満たされている。`pod.rs:1537-1547` で `summary_usage_tracker = Arc::new(UsageTracker::new())` を作り、`on_usage` で `record_usage(event)`、`CompactWorkerInterceptor::pre_llm_request` 内で `usage_tracker.note_request(context.len())` を呼ぶ (`compact/worker.rs:262-272`)。
- 要件3「`compact_worker_max_input_tokens` の意味を「現在占有量しきい値」に変更し doc 更新」: 満たされている。`crates/manifest/src/lib.rs:324`、`crates/manifest/src/defaults.rs:39-43`、`docs/compaction.md:143`、`docs/manifest.toml:215` がすべて「現在占有」「累計ではない」表現に揃っている。フィールド名は維持され、後方互換 shim は入っていない (要件通り)。
- 要件4「`compact_worker_max_turns: Option<u32>` を新設し `Worker::set_max_turns` 経由で渡す」: 満たされている。`CompactionConfig` (`lib.rs:329-332`)、`CompactionConfigPartial` (`config.rs:118-119`)、`defaults::COMPACT_WORKER_MAX_TURNS = Some(20)` (`defaults.rs:46-48`)、`pod.rs:1548-1550` で `summary_worker.set_max_turns(...)`。manifest 側の merge / TryFrom / TOML パースもテスト付きで通っている (`config.rs:960-983`, `lib.rs:521-546`)。
- 要件5「後方互換 shim 無し」: 満たされている。フィールド名は変更しないため旧 manifest はそのまま新セマンティクスで読まれ、deprecated alias 等は導入されていない。
- 完了条件1「cache hit 込み prefix がフルカウントされない」: 単体テスト `compact_worker_interceptor_uses_occupancy_not_cumulative_usage` (`compact/worker.rs:301-328`) で「2 回 100-token 入力 → 累積 200 でも occupancy は最新の 100 のままで cap=150 を通る」を確認。
- 完了条件2「`compact_worker_max_turns` で打ち切られる」: `set_max_turns` のロジック自体は llm-worker 側の既存挙動 (`worker.rs:1045-1050`) に乗るのみで、本チケットでは新規ロジックを足していない。配線テスト (manifest 側のパース) は入っているが、compact 経由で実 abort する pod-level の統合テストは無し。Trivial wiring なのでブロッキングではないが、後で run-level テストを足すと安心。
- 完了条件3「`Pod::total_tokens()` と同じ算出経路」: 満たされている。`pod.rs:148` (`llm_worker::token_counter::total_tokens(self.history(), &usage)`) と `compact/worker.rs:265` (`llm_worker::token_counter::total_tokens(context, &records)`) が同じ関数を経由する。compact 側は per-request の prune 後 `request_context` を渡すため、メイン側と完全一致ではないが、`pre_llm_request` 時点でのその後 LLM に投げる prefix 占有量という意味では妥当 (むしろ正確)。

## アーキテクチャ・スコープ

- 修正は `compact/worker.rs` + `pod.rs` の compact 構築 + manifest 1 フィールド追加に閉じており、ticket 影響範囲表と完全一致。`crates/pod/src/compact/usage_tracker.rs` への変更は「`records()` メソッドを `pub(crate)` で追加して drain せず読む」のみで、API 拡張は最小。
- `UsageTracker` は元々「per-LLM-request 計測のペアリング・drain は Pod が persist 時に行う」モデル。compact worker は drain せず read-only で snapshot を覗くだけなので、メインの persist セマンティクスを汚さない。compact worker のループ内で記録しても drain しないままワーカーが破棄される (`pod.rs:1530-1551` で関数ローカル) ため、メインの `Pod::usage_tracker` (turn 永続化用) とは完全に独立した別インスタンスで、相互干渉しない。設計として綺麗。
- `note_request` をメインでは `UsageTrackingHook` (`pod.rs:46-59`) として `pre_llm_request` Hook で呼んでいるのに対し、compact 側は `CompactWorkerInterceptor::pre_llm_request` 内で直接呼んでいる。Worker のコール順 (Hook → Interceptor → stream → on_usage) を踏まえると `note_request` は `record_usage` より前に呼ばれていれば意味的に等価で、両者とも同じ tick で呼ばれるため不整合は無い。ただし「メイン: Hook で `note_request`、Interceptor で別判定」「compact: Interceptor で両方」と配線パターンが分岐している点は将来読む人が混乱するかも。今のままでも動くが、補足コメントを Pod 側か Interceptor 側に入れておくと親切。
- ループ深さは `Worker::set_max_turns` 既存機構の流用。新規ロジック無し、コードベースを歪めない選択。

## 指摘事項

### Blocking
無し。

### Non-blocking / Follow-up
- `pod.rs:1548-1550` の `if compact_worker_max_turns.is_some() { summary_worker.set_max_turns(compact_worker_max_turns); }` は不要な分岐。`set_max_turns` は `Option<u32>` を取り、`None` を渡しても初期値 `None` を上書きするだけで no-op (`crates/llm-worker/src/worker.rs:1369-1371` + `worker.rs:1181`)。無条件に `summary_worker.set_max_turns(compact_worker_max_turns);` で十分。条件付けることで「`None` だと何もしない」読者バイアスを生んでむしろ読みづらい。
- `crates/pod/src/pod.rs:2178-2201` の `MemoryExtractWorkerInterceptor` は古い累積メトリック方式のまま残っており、コメントには `Mirror of compact::worker::CompactWorkerInterceptor;` と書かれている。今回の変更で「mirror」ではなくなったのでコメントが嘘になっている。memory extract 側のセマンティクス変更は本チケットの範囲外で正しい判断だが、コメント補正 (例: 「以前は CompactWorkerInterceptor のミラーだったが、compact 側は占有量ベースに移行した。memory extract 側は別チケットで追従予定」) を別チケットなり TODO なりで残しておきたい。
- `compact_worker_max_turns` が compact 経由で実際に `Aborted/MaxTurnsReached` を引き起こす経路の pod-level 統合テストは含まれていない。配線そのものはトリビアルで、`set_max_turns` の振る舞いは llm-worker 側で別途テストされている前提。後追いで足すかどうかは判断に任せる。
- デフォルト `COMPACT_WORKER_MAX_TURNS = Some(20)` は `build_summary_prompt` の skeleton + 数回ファイル読みなら十分余裕がある妥当なライン。ただし auto_read_budget (8000) と read_file の典型呼び出しサイズを考えると、深いツールループが起きる前にトークン cap が先に効きやすい設計なので、20 が上限になる場面はまずレアケース。妥当な保険値として OK。

### Nits
- `compact/worker.rs:249-253` の docstring は新しいセマンティクスを正しく説明できている。良。
- `compact/usage_tracker.rs:102-112` `records()` の docstring が「request-time circuit breakers が同じ occupancy projection を見るため」と用途を明示しており、将来の extract worker 側追従の伏線にもなっている。良。
- ユニットテスト `compact_worker_interceptor_uses_occupancy_not_cumulative_usage` (`compact/worker.rs:301-328`) は「累積では落ちる量でも occupancy では通る」ケースをピンポイントで掴んでいて、今回の本質的なバグへの回帰防止として的確。

## 判断

Approve with follow-up — チケットの 4 要件と完了条件 1/3 はすべて根拠つきで満たされている。残るのは (a) `set_max_turns` 呼び出しの不要な分岐、(b) `MemoryExtractWorkerInterceptor` 側のコメント陳腐化と将来の追従、(c) max_turns の pod-level 統合テスト追加、いずれも非ブロッキング。マージ可。
