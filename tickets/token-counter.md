# LlmClient へ Tokenizer の導入

## 背景

現状、トークン数の推定は `len / 4` の荒い近似でしかできていない。
Compact 改善で以下が全て正確なトークン数に依存するため、LlmClient 層で
トークナイザを提供する仕組みが必要になる。

## 動機

正確なトークン数が必要になる箇所:

- Prune の `min_savings` 判定（節約見込みの事前推定）
- Compact 後の `retained_tokens` 切り出し（直近 N トークンの保護）
- Compact worker の auto-read budget 判定
- Protocol の UI 向けトークン表示（将来）

`CompactState::last_input_tokens` は LLM レスポンスから得られる実測値なので
これには影響しない（閾値比較だけなら Tokenizer なしで回る）。ただし
retained_tokens の切り出しは事前にローカルで計算する必要がある。

## 方針

`LlmClient` trait に同期的な `Tokenizer` 取得口を追加する。非同期 API は使わない
（Prune/Compact の判定フローを async にしたくない）。

```rust
pub trait Tokenizer: Send + Sync {
    fn estimate_text(&self, s: &str) -> u64;
    fn estimate_items(&self, items: &[Item]) -> u64;
}

pub trait LlmClient {
    // 既存メソッド...
    fn tokenizer(&self) -> Arc<dyn Tokenizer>;
}
```

- provider ごとに実装。精度は近似で十分（±10% 程度）
- OpenAI/Anthropic 系は `tiktoken-rs` + BPE テーブルベースの近似
- Gemini も近似で対応。厳密値が要る場面があれば後から `count_tokens` API 呼び出しに置き換え可
- `estimate_items` は `Item` の variant を舐めて text/tool name/arguments を足し込む単純実装

## 設計ポイント

- **同期 API**: Prune hook や Compact 判定の中で使う。`async fn` を増やさない
- **Arc 返却**: Worker/Pod で使い回せるよう共有参照
- **近似で十分**: 正確性より呼び出しコストの低さを優先。実測値 (`last_input_tokens`) との二重化で補正される
- **Client ごとの実装**: provider 固有のトークナイザ差異はここで吸収

## 実装対象

- `llm-worker/src/llm_client/tokenizer.rs` (新規) — `Tokenizer` trait 定義
- `llm-worker/src/llm_client/types.rs` or `LlmClient` trait — `fn tokenizer(&self) -> Arc<dyn Tokenizer>`
- provider 実装:
  - `providers/anthropic.rs`
  - `providers/openai.rs`
  - `providers/gemini.rs`
- 依存追加: `tiktoken-rs` （`cargo add` で）

## 依存

- なし（これ自体が前提チケット）

## ブロックする後続

- [compact-improvements.md](compact-improvements.md) — retained_tokens, auto-read budget が依存
