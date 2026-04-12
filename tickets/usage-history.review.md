# usage-history レビュー

## 要件の充足

チケットが定義した要件は全て達成されている:

- **LogEntry::LlmUsage**: session-store のハッシュチェーンに乗る variant として追加。
  `history_len` / `input_total_tokens` / `cache_read_tokens` / `cache_write_tokens` / `output_tokens` の5フィールド
- **RestoredState.usage_history**: `collect_state` の replay で `Vec<UsageRecord>` に時系列順で積まれる。history の構築には影響しない
- **save_usage**: `append_entry` 経由でハッシュチェーンに接続
- **既存ログ互換**: `LlmUsage` entry が無い既存ログを読んでも `usage_history` が空になるだけで壊れない
- **1リクエスト = 1 entry**: Timeline の `pending_usage` + `flush_usage()` で複数 Usage event を集約し、handler には1度だけ発火

## アーキテクチャ

レイヤー分担が明確で、各層の責務が逸脱していない:

| レイヤー | 責務 |
|---------|------|
| scheme (anthropic) | raw → 占有量への正規化。`input_tokens + cache_read + cache_creation` |
| Timeline | 1リクエスト内の複数 Usage event をフィールド単位 latest-non-None でマージ。`flush_usage()` で1度だけ発火 |
| Worker | ストリーム完了・エラー・キャンセルの全パスで `flush_usage()` を呼ぶ |
| UsageTracker (pod) | `note_request(history_len)` と `record_usage(event)` のペアリング。drain で Pod に渡す |
| Pod::persist_turn | drain した records を `save_usage` で session-store に書き出し |

## 指摘と対処

### 1. UsageEvent の doc comment（対処済み）

`UsageEvent.input_tokens` が「占有量（プロンプト全長、キャッシュ込み）」を意味することが
struct と各フィールドの doc comment に明記された。scheme 層での正規化規約も記載済み。

### 2. save_usage の引数が多い（非ブロッカー、未対処）

8引数。`UsageRecord` を直接受け取れば `drain()` の結果をそのまま渡せてシグネチャがきれいになるが、
他の `save_*` 関数がフラットな引数を取るパターンと一貫しているため、統一性の観点では現状でも妥当。
将来フィールドが増えた時点でまとめて `UsageRecord` 受け取りに変えればよい。

## テスト

- `replay_llm_usage_appends_to_usage_history`: 複数 LlmUsage entry の replay で usage_history が正しく積まれ、history.len() に影響しない
- `replay_without_llm_usage_keeps_usage_history_empty`: 既存ログ互換
- `llm_usage_entry_round_trip_via_json`: serde 往復
- `test_convert_usage_includes_cache_in_input_total`: Anthropic の占有量正規化
- `test_usage_aggregation_and_flush`: Timeline の集約 + flush
- UsageTracker: ペアリング、drain、未ペアの drop、複数リクエスト

## 判定

承認。
