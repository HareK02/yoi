# モデル capability の責務を llm-worker 外へ移す

## 背景

`llm-worker` は「wire scheme の送受信」に専念する低レベル基盤に留める方針。にもかかわらず、現状は各 scheme に `capability.rs` が同居していて、`claude-*` / `gpt-5` prefix / `gemini-2.5` prefix / `codex-` prefix 等の個別モデル ID 知識が `llm-worker` に閉じ込められている。

該当箇所:

- `crates/llm-worker/src/llm_client/scheme/anthropic/capability.rs::lookup`
- `crates/llm-worker/src/llm_client/scheme/openai_chat/capability.rs::classify` / `lookup`
- `crates/llm-worker/src/llm_client/scheme/openai_responses/capability.rs::lookup`
- `crates/llm-worker/src/llm_client/scheme/gemini/capability.rs::lookup`
- `Scheme::capability_for(model_id) -> Option<ModelCapability>` trait メソッド

「モデル ID から能力を決める」は wire 実装の責務ではない。カタログ/プロバイダ構築層(高レベル側)の責務で、ここに居るのは層の漏出。新モデル(`gpt-6` 等)が出るたびに `llm-worker` を触る構造は抽象が壊れている。

## 要件

1. **`Scheme::capability_for` の廃止**: trait から削除。`llm-worker` は wire-level の安全側フォールバックである `default_capability()` のみ持つ。

2. **モデル知識を `crates/provider` へ移す**:
   - `crates/provider/src/capability.rs`(または `capability/` モジュール)を新設
   - 現 `scheme/*/capability.rs` の `lookup`(および `classify`)を移植
   - 公開 API: `pub fn lookup(scheme: SchemeKind, model_id: &str) -> Option<ModelCapability>`

3. **`build_client` の capability 解決順は維持**:
   1. `ModelConfig.capability` 明示指定
   2. `provider::capability::lookup(scheme, model_id)` (移設先)
   3. `scheme.default_capability()` (`llm-worker` に残る)

4. **`llm-worker/examples/*` の追従**: `scheme.capability_for` を使っている箇所(`worker_cli` / `worker_cancel_demo` / `record_test_fixtures`)は `scheme.default_capability()` か明示 `ModelCapability` に置換する。examples を provider に依存させない。

5. **テストの移設**: scheme 側 capability テストは provider 側に移す(テーブル本体と一緒に移動)。

6. **完了時の動作**: `cargo build --workspace` が通り、`test_pod.codex.local.toml` 経由の疎通テストが引き続き成功する。

## 設計判断

### scheme 側に `default_capability()` を残す理由

wire format 固有の保守的 default (例: OpenAI 互換は Parallel tool call, JsonSchema structured output) は wire 層の知識で、モデル固有ではない。「この scheme では何が最低限送れるか」を示す安全側の宣言なので `llm-worker` に残す。

### examples を provider に依存させない

worker_cli 等は scheme の使い方を示す最小例。provider クレートに依存させると低レベル例として壊れるし、循環依存の気配も出る。examples では `scheme.default_capability()` を使い、モデル固有最適化が必要なら `ModelCapability` 手組みで済ませる。

### `llm-provider-catalog` との関係

カタログチケットは「プロバイダと代表モデル ID の提示」が主題で、capability は意図的に載せない判断だった。本チケットはその決定を変えない。`provider::capability::lookup` は provider 層のうちカタログとは別の関数として置く(将来的にカタログが capability ヒントを提供する余地は残すが、本チケットでは扱わない)。

### 新モデル追加の運用

`provider::capability::lookup` が「知ってるモデル ID」のテーブルを持つ設計は維持する(本チケットの焦点は場所の正しさ)。新モデル対応はテーブル追記で済むが、新スキーマ/新プロバイダ経路が出たときに llm-worker を触らずに済むのが改善点。

## Scope 外

- ランタイム capability 検出 (OpenAI `/v1/models` や ChatGPT backend の動的モデル列挙)
- `providers.toml` カタログ連携 (`llm-provider-catalog` チケット側)
- prefix match から厳密マッチへの切替(別論点、今回は挙動保存)

## 影響範囲

- `crates/llm-worker/src/llm_client/scheme/mod.rs`: `capability_for` trait メソッド削除
- `crates/llm-worker/src/llm_client/scheme/*/scheme_impl.rs`: `capability_for` impl 削除
- `crates/llm-worker/src/llm_client/scheme/*/capability.rs`: `lookup`/`classify` 削除、`default_capability` のみ残す
- `crates/provider/src/capability.rs`: 新設(テーブル統合)
- `crates/provider/src/lib.rs`: `build_client` の capability 解決経路を更新
- `crates/llm-worker/examples/*`: `capability_for` 呼び出し除去

## 依存

- `llm-model-config` 完了済
- `llm-scheme-openai-responses` 完了済
- `llm-auth-codex-oauth` 完了済

## Review
- 状態: Approve
- レビュー詳細: [./llm-capability-ownership.review.md](./llm-capability-ownership.review.md)
- 日付: 2026-04-21
