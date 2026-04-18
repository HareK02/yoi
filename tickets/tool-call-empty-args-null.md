# 引数なし tool 呼び出しで `arguments = "null"` が記録される不具合

## 背景

引数を取らないツール（例: `ListPods`）を Anthropic の Claude が呼び出したとき、次ターンで履歴を送り返す際に Anthropic API が以下のエラーで 400 を返す：

```
messages.N.content.0.tool_use.input: Input should be a valid dictionary
```

実環境で `cargo run -p pod` + TUI / API 経由で `ListPods` を呼ぶと再現する。セッション jsonl には tool 呼び出しが以下の形で記録されている：

```json
{"type":"tool_call","call_id":"toolu_...","name":"ListPods","arguments":"null"}
```

`arguments` が `"null"` 文字列になっており、次ターンで Anthropic に送る `tool_use.input` が JSON `null` として serialize されてしまうことが原因。

## 原因

`crates/llm-worker/src/timeline/tool_call_collector.rs:87-88`:

```rust
let input = serde_json::from_str(&scope.input_json_buffer)
    .unwrap_or(serde_json::Value::Null);
```

Anthropic は引数なしのツール呼び出しでは `input_json_delta` を一度も送らない。その結果 `input_json_buffer` が空文字 `""` のまま stop イベントに到達し、`from_str("")` が失敗して `Value::Null` に fallback する。

この `Null` が `worker.rs:499` で `Item::tool_call_json(..., Value::Null)` として履歴に保存され、`Value::Null.to_string()` = `"null"` が `arguments` フィールドに残る。

次ターンで `anthropic/request.rs:174-175` が history → request body 変換する際：

```rust
let input = serde_json::from_str(arguments)
    .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new()));
```

`"null"` は valid JSON として parse 成功するため fallback が効かず、`Value::Null` のまま `tool_use.input` に入り API に送信される。Anthropic の tool_use.input は object 必須なので拒否される。

## 修正方針

ルート修正を `tool_call_collector.rs` に入れる。引数なし / パース失敗の場合は `Value::Object(Map::new())`（= `{}`）にする：

```rust
let input = if scope.input_json_buffer.is_empty() {
    serde_json::Value::Object(serde_json::Map::new())
} else {
    serde_json::from_str(&scope.input_json_buffer)
        .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new()))
};
```

加えて防御層として `anthropic/request.rs:174` で parse 結果が object でない場合も `{}` に正規化する。既に `arguments = "null"` として保存済みの古いセッションが resume されたときに回復できるようにするため。

## 影響範囲

- `crates/llm-worker/src/timeline/tool_call_collector.rs`: 空バッファ時の default を `Value::Object` に
- `crates/llm-worker/src/llm_client/scheme/anthropic/request.rs`: parse 結果が非 object の場合の正規化
- `crates/llm-worker/src/worker.rs:576-577` の同様の parse でも非 object を正規化（防御）
- OpenAI / Gemini の request.rs でも同等の問題があるか確認、必要なら同じ修正を入れる

## 完了条件

- 引数なしツール（`ListPods` など）を呼んだ直後のセッション jsonl で `arguments` が `"{}"` になる
- 同一セッション内で引数なしツールを呼んでから次ターンを開始しても 400 エラーが出ない
- `"arguments":"null"` が残っている既存セッションを resume しても 400 エラーが出ない
- `tool_call_collector.rs` に「空バッファ → `{}`」を検証するテストを追加
- 該当パスの回帰テストを単体テストで担保

## 範囲外

- LLM 側が「無意味に `null` を `input` に入れてくる」ケースの検出・警告（そもそもプロバイダから来ないはず）
- OpenAI / Gemini の同等検証（確認だけ行い、問題あれば別チケット化）
- 既存セッション jsonl の自動修復スクリプト（resume 時の defensive 正規化で十分）
