# Review: session-store / llm-worker 型責務の整理

## 前提・要件の確認

### 1. `UsageRecord` を llm-worker に移動
- 移動先: `crates/llm-worker/src/usage_record.rs:9` に `UsageRecord` を新規定義、`crates/llm-worker/src/lib.rs:54,61` で `pub mod` + `pub use` 済み
- session-store 側: `crates/session-store/src/lib.rs:42` で `pub use llm_worker::UsageRecord;` の互換 re-export
- session-store 内部: `crates/session-store/src/session_log.rs:12` で `use llm_worker::{UsageRecord, WorkerResult};` に切替済み（旧定義は削除）
- pod の参照経路更新: `crates/pod/src/pod.rs:9`, `crates/pod/src/compact/usage_tracker.rs:19`, `crates/pod/src/ipc/interceptor.rs:19` がいずれも `llm_worker::UsageRecord` 経由
- `LogEntry::LlmUsage` は inline fields のまま（`session_log.rs:160`）— 方針通り
- 充足

### 2. `token_counter` を llm-worker に移動
- 汎用部分: `crates/llm-worker/src/token_counter.rs` に `prefix_bytes`, `tokens_at`, `total_tokens`, `total_tokens_at`, `item_bytes` 移動済み。`EstimateSource` / `TokenEstimate` も llm-worker に集約
- compact 専用部分: `crates/pod/src/compact/token_counter.rs` に `SplitPoint`, `split_for_retained_impl`, `tool_result_content_bytes`, `savings_for_prune_impl` 残置
- Pod の薄ラッパー: `total_tokens` (`compact/token_counter.rs:146`), `total_tokens_at` (`:157`), `split_for_retained` (`:165`) はいずれも `llm_worker::token_counter::*` 呼び出しの薄ラッパーに
- 外部呼出経路: `pod/src/ipc/interceptor.rs:24,85` で `use llm_worker::token_counter::total_tokens;` を直接利用（pod 経由を回避できる）
- pod の lib re-export (`pod/src/lib.rs:14`) は維持されており、外部 API の互換が崩れていない
- 充足

### 3. `Outcome` 廃止 + `RunCompleted` / `RunErrored` への分解
- `WorkerResult` の derive: `crates/llm-worker/src/worker.rs:69-70` に `#[derive(Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]` + `#[serde(rename_all = "snake_case")]` 追加済み
- `Outcome` enum は完全削除（`session_log.rs` 内に痕跡なし、grep 確認済み）
- `LogEntry::RunCompleted` / `LogEntry::RunErrored` 2 variants で表現（`session_log.rs:128-142`）。両方とも audit-only metadata のまま
- `collect_state` の対応 arm（`session_log.rs:254-259`）はチケット指示通り `state.last_run_interrupted = *interrupted` のみ
- 関数分割: `save_run_completed` (`session.rs:242`) / `save_run_errored` (`session.rs:266`) の 2 関数。lib.rs の `pub use` (`lib.rs:40`) も追従
- pod 側 match: `pod/src/pod.rs:964-985` で `Ok(r) => save_run_completed / Err(e) => save_run_errored` 化
- テスト更新: `session_test.rs:118-138`, `fs_store_test.rs:34`, `session_log.rs` 内の `replay_full_turn` 等が `RunCompleted` 参照に更新
- 既存ログ互換: variant tag 破壊的変更（`run_outcome` → `run_completed` / `run_errored`）。チケットの判断「実運用ログがほぼ無い前提なら破壊的変更で OK」に沿う
- 充足

### 4. `Locked` / `CacheUnlocked` 関連 API 削除
- `LogEntry::Locked` / `LogEntry::CacheUnlocked` variants 削除、grep で痕跡なし（session-store 配下）
- `save_cache_locked` / `save_cache_unlocked` 関数削除、`lib.rs` の re-export からも削除
- `RestoredState.locked_prefix_len` field 削除、`collect_state` の match arm 削除
- 関連 unit test 削除（`replay_cache_lock_unlock`, `session_cache_lock_unlock_logged` 等）
- `replay_empty` の `assert_eq!(state.locked_prefix_len, 0)` も削除済み
- Worker 内部の `locked_prefix_len: usize` は session-store と無関係 — 残置で問題なし（チケット注記と一致）
- 充足

## アーキテクチャ・スコープ
- 依存方向: pod → llm-worker、pod → session-store、session-store → llm-worker を維持。Cargo.toml で確認、循環なし
- llm-worker の階層性: `usage_record` / `token_counter` の追加は LLM call の per-call measurement と pure な token accounting のみで、上位ドメイン（compact 等）を持ち込んでおらず低レベル基盤の方針に沿う
- crate 名前付け: 新規ファイル追加のみで、新規 crate なし
- ScopedFs 等のスクリプティング計画への影響なし
- LLM provider policy への影響なし
- 変更範囲: チケット記載の 4 項目に正確に対応、範囲外の改変は見当たらない
- ビルド & テスト: `cargo check --workspace --all-targets` 新規 warning なし。session-store 8+7+13 件、llm-worker token_counter 4 件、pod 全テスト pass を再確認

## 指摘事項

### Non-blocking / Follow-up
- なし

### Nits
- `crates/pod/src/ipc/interceptor.rs:274` のテスト用ヘルパー doc comment が `total_tokens_impl` を参照したまま。実際の関数は `llm_worker::token_counter::total_tokens` にリネーム済みなので追従が望ましい
- `crates/pod/src/compact/token_counter.rs:34` の `split_for_retained_impl` は pod ローカルなヘルパー（公開 API は `Pod::split_for_retained`）なので `_impl` サフィックスは慣習的に許容範囲。ただし `total_tokens_impl` → `total_tokens` のリネーム方針と揃えるなら `split_for_retained_inner` 等に揃える余地あり（現状のままで実害なし）

## 判断
Approve — チケットに記載された 4 項目すべてが過不足なく実装され、既存テストはパス、依存方向と層責務も維持されている。残課題は doc comment 1 行のみで、ブロッキングではない。
