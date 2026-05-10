# Memory: extract worker のサーキットブレーカーを占有量ベースに統一

## 背景

`compact-worker-occupancy-cap` で compact worker のサーキットブレーカーを `UsageTracker` + `llm_worker::token_counter::total_tokens` ベースに切り替えたが、同じ設計上のバグを抱えた `MemoryExtractWorkerInterceptor` が `crates/pod/src/pod.rs:2180-2199` に残っている。実装コメントには `Mirror of compact::worker::CompactWorkerInterceptor;` と書かれており、compact 側が直った今、コメントも事実と乖離している。

具体的には:

- `MemoryExtractWorkerInterceptor` は `input_so_far: AtomicU64` に `UsageEvent.input_tokens` を `fetch_add` し、`pre_llm_request` で累積値が `max_input_tokens` を超えたら `Cancel` する (`pod.rs:1930-1942` で配線、`pod.rs:2186-2198` で判定)。
- `UsageEvent.input_tokens` は cache hit を含む prompt prefix 全体の占有量 (`crates/llm-worker/src/llm_client/event.rs:76-94`)。同じ prefix を毎ターンフルカウントするため、cache が 99% ヒットしても累積値は context size × ターン数で増える。
- 結果は compact と同じ：cap が「現在占有量」ではなく「累積（≒ context × turns）」を測っており、設計と整合しない。

extract worker は compact ほどツールループが深くない（`extract.write_extracted_tool` のみ、ファイル探索なし）ため誤発火頻度は compact より低いものの、構造的な誤りは同じであり、放置すると今後ツールが増えた瞬間に再発する。

メイン Pod のしきい値群と compact worker (修正済み) はどちらも `Pod::total_tokens()` (`pod.rs:146-149`) 経由で「現在の占有量」を見ており、extract worker だけ別ロジックという歪みも残っている。

## 方針

`MemoryExtractWorkerInterceptor` を、compact 側と同じ `UsageTracker` + `llm_worker::token_counter::total_tokens` 機構に乗せ替える。累積メトリックは廃止する。compact と対称に `extract_worker_max_turns` を新設し、`Worker::set_max_turns` 経由で extract worker に伝える。

## 要件

- `MemoryExtractWorkerInterceptor` の `pre_llm_request` を `llm_worker::token_counter::total_tokens(context, &records).tokens > max_input_tokens` ベースに置き換える。`input_so_far: AtomicU64` の累積パスは廃止。
- Extract worker にも `UsageTracker` を持たせ、`pre_llm_request` で `note_request(context.len())`、`on_usage` で `record_usage(event)` する。compact worker (`pod.rs:1535-1547` 周辺) と同じ配線パターン。
- `extract_worker_max_input_tokens` の意味を「extract worker 側の現在占有量しきい値」に変更し、ドキュメント (`crates/manifest/src/lib.rs:111-115`, `crates/manifest/src/defaults.rs:54-56`, 関連 `docs/`) を更新する。デフォルト値 30000 は新セマンティクスで再評価可能だが、変更必須ではない。
- `MemoryConfig` に `extract_worker_max_turns: Option<u32>` を追加し、cascade (`crates/manifest/src/config.rs`) に通したうえで extract worker 構築箇所 (`pod.rs:1924-1942`) で `extract_worker.set_max_turns` に渡す。デフォルトは要検討（仮: `Some(8)` — extract は本来 1-2 ターンで終わる軽量 worker のため compact より小さく）。
- compact 側で消した「Mirror of `CompactWorkerInterceptor`」というコメントを削除し、独立した interceptor として doc を整える（実体はほぼ同じだが、ロジック共有のための共通化はやらない: subsystem ごとにメッセージとしきい値が異なる前提を維持）。
- 後方互換 shim は入れない。`extract_worker_max_input_tokens` はフィールド名を維持しつつセマンティクスだけ差し替える。

## 完了条件

- 通常の extract 実行で、cache hit 込みの prefix がフルカウントされなくなり、現実的な extract 入力 (compaction 区間の skeleton) で cap に当たらない。
- extract worker のループが暴走するケースで、`extract_worker_max_turns` により打ち切られる。
- `Pod::total_tokens()` / `CompactWorkerInterceptor` / `MemoryExtractWorkerInterceptor` の 3 者が同じ算出経路 (`llm_worker::token_counter::total_tokens`) を使っている。
- `MemoryExtractWorkerInterceptor` のユニットテストで「累積では落ちる量でも occupancy では通る」「占有量が cap を超えたら cancel」の 2 パターンが covered されている。

## 範囲外

- `extract_threshold`（extract 発火条件）のセマンティクス変更。これは「ポインタ以降の cumulative input tokens」で別概念。
- Consolidation 側の同等変更（consolidation worker は別の lock + iteration 機構で動いており、interceptor 設計が異なる。必要なら別チケット）。
- `compact_threshold` / `compact_request_threshold` 自体のセマンティクス変更。
- 共通 `OccupancyInterceptor` を新設して compact / extract で共有する抽象化。サブシステム個別のメッセージ・cap・将来の挙動分岐を残せるよう、現状の重複構造を維持。

## 影響範囲

- `crates/pod/src/pod.rs`:
  - `MemoryExtractWorkerInterceptor` 定義 (`pod.rs:2180-2199`) を occupancy ベースに書き換え、`input_so_far` フィールドを `usage_tracker: Arc<UsageTracker>` に置換。
  - extract worker 構築箇所 (`pod.rs:1924-1942`) で `UsageTracker` 配線、`set_max_turns` 呼び出し追加。
- `crates/manifest/src/lib.rs`:
  - `MemoryConfig::extract_worker_max_input_tokens` の doc を新セマンティクスに更新。
  - `extract_worker_max_turns: Option<u32>` フィールド追加 + デフォルト値関数。
- `crates/manifest/src/config.rs`: `MemoryConfigPartial` への追加と cascade 解決。
- `crates/manifest/src/defaults.rs`: `MEMORY_EXTRACT_WORKER_MAX_INPUT_TOKENS` の doc 更新、`MEMORY_EXTRACT_WORKER_MAX_TURNS` 定数の追加。
- `docs/manifest.toml` 等の memory 設定例: 新フィールドと意味の更新。
- `crates/pod/src/pod.rs` 周辺もしくは新規モジュールでのユニットテスト（compact 側で追加した 2 パターンを extract 用に）。
