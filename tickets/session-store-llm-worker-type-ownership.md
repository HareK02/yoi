# session-store / llm-worker 型責務の整理

## 背景

session-store の `LogEntry` 周りで「llm-worker の概念を session-store 内に二重定義 / inline flatten している」「未使用の variants が残っている」状態が複数ある。`memory-phase1-extract` の作業中に整理対象として浮上したが、本筋とは別軸なので独立チケットに切り出す。

依存方向は崩さない: pod → llm-worker、pod → session-store、session-store → llm-worker は維持。

## 要件

### 1. `UsageRecord` を llm-worker に移動

- 現状: `crates/session-store/src/session_log.rs` 内で `pub struct UsageRecord { history_len, input_total_tokens, cache_read_tokens, cache_write_tokens, output_tokens }` を定義
- 本性: 「ある history prefix 長で 1 リクエスト送ったときの計測スナップショット」 = LLM call に紐づく per-call measurement。永続化が本質ではない
- 移動先: `crates/llm-worker/src/usage_record.rs` (or `llm_client/usage.rs`)。`UsageEvent` (provider stream イベント) と隣接させる
- session-store 側: `pub use llm_worker::UsageRecord` で互換 re-export。`LogEntry::LlmUsage` は inline fields のままで良い (中身が `UsageRecord` 1 個分の field 列に対応している)
- pod 側: import 経路だけ更新

### 2. `token_counter` を llm-worker に移動

- 現状: `crates/pod/src/compact/token_counter.rs` に `prefix_bytes`, `tokens_at`, `total_tokens_impl`, `total_tokens_at_impl`, `split_for_retained_impl`, `tool_result_content_bytes` が同居
- consumer も増えている: 当初は compact だけだったが、memory phase 1 trigger (`Pod::tokens_added_since` → `total_tokens_at(now) - total_tokens_at(pointer)` の差分計算) でも同じ accounting を使うようになった。`compact::` 名前空間下にあるのが事実とそぐわない
- 移動方針:
  - **汎用部分** (`prefix_bytes`, `tokens_at`, `total_tokens_impl`, `total_tokens_at_impl`) を `crates/llm-worker/src/token_counter.rs` に移す。`Item` も llm-worker、`UsageRecord` も llm-worker に来るので素材が揃う
  - **compact 専用部分** (`split_for_retained_impl`, `tool_result_content_bytes`) は pod 側に残す (compact / prune だけが consumer)
- pod 側の `Pod::total_tokens()` / `Pod::total_tokens_at()` / `Pod::split_for_retained()` メソッドは llm-worker の関数を呼ぶ薄ラッパーに (現在は `compact::token_counter::*_impl` を呼んでいる、import 経路だけが変わる)
- これにより phase 1 trigger と将来の usage metrics が `use llm_worker::token_counter::...` で参照できるようになり、`compact::` 経由の不自然な依存が解消される

### 3. `Outcome` 廃止 + `LogEntry::RunCompleted` / `RunErrored` に flat 展開

- 現状: `crates/session-store/src/session_log.rs` の `Outcome` enum が `WorkerResult` の 4 variants (Finished / Paused / LimitReached / Yielded) を再定義した上に `Error { message: String }` を追加した形。`LogEntry::RunOutcome { outcome: Outcome, interrupted: bool }` で wrap されてる
- 当初設計の意図 (`docs/persistence.md` の元コミット 2026-04-05): `RunOutcome` は **「audit-only metadata、replay 分岐には使わない」** と明記されていた。後から log viewer 等の consumer ができる前提で「書く側だけ整えた」状態。現在も replay は `interrupted: bool` しか参照しない (`session_log.rs:294`)
- 問題点: WorkerResult の 4 variants が session-store 側で二重定義されている / `Outcome` 中間層が JSON / Rust 両方で余分なネストを生む / variant 名 (`RunOutcome`) と enum 名 (`Outcome`) が重複
- 動機: pod の `handle_worker_result` で `Result<WorkerResult, WorkerError>` を 1 record に永続化する必要がある。`WorkerError` は `ClientError` (reqwest 等) を wrap していて `Serialize` 不可能なので、エラー側は `message: String` に lossy 変換するしかない (この事情は変わらない)
- 改修方針:
  - `llm_worker::WorkerResult` に `#[derive(Serialize, Deserialize)]` + `#[serde(rename_all = "snake_case")]` を追加
  - session-store 側の `Outcome` enum を **完全削除**
  - `LogEntry::RunOutcome` を 2 variants に分解 (audit metadata の意図は保持):
    ```rust
    pub enum LogEntry {
        // ...

        /// run() / resume() が WorkerResult で正常終了した。
        /// 当初設計どおり audit-only: replay は `interrupted` のみ反映。
        RunCompleted {
            ts: u64,
            interrupted: bool,
            result: llm_worker::WorkerResult,
        },

        /// run() / resume() が WorkerError で終了した。
        /// WorkerError は Serialize 不可なので message のみ lossy 保持。
        /// audit-only: replay は `interrupted` のみ反映。
        RunErrored {
            ts: u64,
            interrupted: bool,
            message: String,
        },

        // ...
    }
    ```
  - `save_outcome` を `save_run_completed` / `save_run_errored` の 2 関数に分割 (or `save_run_outcome(result: &Result<WorkerResult, WorkerError>)` の helper を 1 つだけ持って内部で振り分け、どちらでも)
  - pod の `match` (`pod.rs:967` 付近) を `Ok(r) => save_run_completed(.., r) / Err(e) => save_run_errored(.., e.to_string())` に
  - `collect_state` の対応 match arm 2 つに分けるが、どちらも `state.last_run_interrupted = *interrupted` だけ
- 既存ログ互換: variant tag が変わる (`run_outcome` → `run_completed` / `run_errored`) ので JSON 形式が変わる。v1 ログを読む経路があるなら custom deserializer か migration が必要 (実運用ログがほぼ無い前提なら破壊的変更で OK、判断はチケット着手時)

### 4. `LogEntry::Locked` / `LogEntry::CacheUnlocked` および周辺 API を削除

- 現状: variants 自体は残っているが、**書き手 (caller) が存在しない**
  - `save_cache_locked` / `save_cache_unlocked` は `pub` 公開だが session-store 外の呼び出しゼロ
  - Pod は `worker.set_cache_anchor(...)` を in-memory で操作するだけで永続化していない
  - `RestoredState.locked_prefix_len` も誰も読んでいない
- 削除対象:
  - `LogEntry::Locked` / `LogEntry::CacheUnlocked` variants
  - `save_cache_locked` / `save_cache_unlocked` 関数 (lib.rs の re-export 含む)
  - `RestoredState.locked_prefix_len` field
  - `collect_state` の対応 match arm
  - 関連 unit test
- 既存ログ互換: 上述の通り書き手不在なので既存ログにエントリは入っていないはず。念のため `serde(other)` 等で未知 variant を skip する救済層を入れるかは判断

## 範囲外

- `LogEntry::TurnEnd` の `usize` flatten (Worker.turn_count() の永続化) — 重複というほどではないので触らない
- pod の cache anchor 永続化を実装する話 — 必要性が出てから別途
- session-store の独立した一般化 (memory ドメイン以外の Extension 用途展開) — 必要が出てから別途

## 完了条件

- `UsageRecord` が llm-worker から `pub` され、session-store / pod の参照経路が更新されて workspace 全テスト pass
- token_counter の汎用部分が llm-worker 配下にあり、pod / 将来の memory phase 1 から `use llm_worker::token_counter::...` で参照できる
- `Outcome` enum が削除され、`LogEntry::RunCompleted { result: WorkerResult }` / `LogEntry::RunErrored { message }` の 2 variants で表現される。`WorkerResult` の 4 variants は llm-worker 単一情報源
- `Locked` / `CacheUnlocked` 関連 variants / 関数 / fields が削除されてビルド & テストが通る
- 既存 compact / prune / phase 1 trigger の挙動に回帰がない (token accounting の数値が変わらない、Outcome serialization の往復が成立する)

## 参照

- `crates/session-store/src/session_log.rs` (LogEntry, RestoredState, UsageRecord, Outcome)
- `crates/session-store/src/session.rs` (save_outcome, save_cache_*, save_usage)
- `crates/llm-worker/src/worker.rs` (WorkerResult, WorkerError, set_cache_anchor)
- `crates/pod/src/compact/token_counter.rs` (移動元)
- `crates/pod/src/pod.rs` (handle_worker_result の Outcome 構築箇所、Pod::total_tokens 経路)
- `docs/persistence.md` (元設計の意図: RunOutcome は audit-only)

## Review
- 状態: Approve
- レビュー詳細: [./session-store-llm-worker-type-ownership.review.md](./session-store-llm-worker-type-ownership.review.md)
- 日付: 2026-04-28
