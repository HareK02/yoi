# テスト Fixture 仕様

## 概要

テスト用 fixture は、実 API のストリーミング応答を JSONL 形式で録画したファイル。
`MockLlmClient::from_fixture()` でロードし、API キー不要・決定的なテスト実行を実現する。

## ファイル形式

```
{メタデータ行: JSON}
{イベント行: JSON}
{イベント行: JSON}
...
```

- **1行目**: メタデータ (`timestamp`, `model`, `description`)
- **2行目以降**: 録画イベント (`elapsed_ms`, `event_type`, `data`)
  - `data` フィールドに `Event` の JSON 文字列が入る

## ファイル配置

```
crates/llm-worker/tests/fixtures/
  anthropic/
    simple_text.jsonl
    tool_call.jsonl
    long_text.jsonl
  openai/
    simple_text.jsonl
    tool_call.jsonl
    long_text.jsonl
  gemini/
    simple_text.jsonl
    tool_call.jsonl
    long_text.jsonl
  ollama/
    simple_text.jsonl
    tool_call.jsonl
    long_text.jsonl
```

## シナリオ定義

### simple_text

単純なテキスト応答。

| 項目 | 値 |
|---|---|
| ファイル名 | `simple_text.jsonl` |
| system prompt | `"You are a helpful assistant. Be very concise."` |
| user message | `"Say hello in one word."` |
| max_tokens | 50 |
| ツール | なし |

**期待パターン**:
- `BlockStart(Text)` が1つ以上
- `BlockDelta(Text)` が1つ以上
- `BlockStop(Text)` が1つ以上
- 応答が短い（1単語程度）

**用途**: 基本的なストリーミング動作、Timeline テキスト収集、Worker の単純な run 完了

### tool_call

ツール呼び出しを含む応答。

| 項目 | 値 |
|---|---|
| ファイル名 | `tool_call.jsonl` |
| system prompt | `"You are a helpful assistant. Use tools when appropriate."` |
| user message | `"What's the weather in Tokyo? Use the get_weather tool."` |
| max_tokens | 200 |
| ツール | `get_weather(city: string)` |

**期待パターン**:
- `BlockStart(ToolUse)` を含む
- ToolUse ブロック内に `tool_call_id`, `name: "get_weather"` がある
- tool input JSON に `"city"` キーを含む

**用途**: ToolCallCollector、Worker のツール実行フロー、Session の ToolResults ログ記録

### long_text

長文テキスト応答。

| 項目 | 値 |
|---|---|
| ファイル名 | `long_text.jsonl` |
| system prompt | `"You are a creative writer."` |
| user message | `"Write a short story about a robot discovering a garden. It should be at least 300 words."` |
| max_tokens | 1000 |
| ツール | なし |

**期待パターン**:
- `BlockDelta(Text)` が複数（ストリーミングチャンク）
- 最終テキストが 300 語以上

**用途**: ストリーミングの分割配信検証、Subscriber のデルタ受信テスト

## 共通検証項目

全 fixture に対して以下を検証する（`assert_*` ヘルパー関数群）:

- `assert_events_deserialize` — 全イベントが `Event` にデシリアライズできる
- `assert_event_sequence` — BlockStart → BlockDelta → BlockStop の基本シーケンス
- `assert_usage_tokens` — `Usage` イベントが含まれる
- `assert_timeline_integration` — Timeline に流してテキスト収集できる

## 録画手順

### 前提

- API キーが環境変数に設定されていること
- `crates/llm-worker` ディレクトリで実行

### コマンド

```bash
# 単一シナリオ録画
ANTHROPIC_API_KEY=... cargo run --example record_test_fixtures -- -s simple_text

# 全シナリオ録画
ANTHROPIC_API_KEY=... cargo run --example record_test_fixtures -- --all

# プロバイダー指定
OPENAI_API_KEY=... cargo run --example record_test_fixtures -- --all -c openai
GEMINI_API_KEY=... cargo run --example record_test_fixtures -- --all -c gemini

# モデル指定
ANTHROPIC_API_KEY=... cargo run --example record_test_fixtures -- --all -m claude-sonnet-4-20250514
```

### シナリオ定義の場所

`crates/llm-worker/examples/record_test_fixtures/scenarios.rs`

新しいシナリオを追加する場合はこのファイルに `TestScenario` を追加し、
`scenarios()` 関数の返り値に含める。

## 録画後の確認チェックリスト

録画後、テストに組み込む前に以下を手動確認する:

- [ ] JSONL の各行が valid JSON か（`jq . < fixture.jsonl` で確認）
- [ ] 1行目にメタデータ（`timestamp`, `model`, `description`）が入っているか
- [ ] simple_text: `BlockStart` → `BlockDelta` → `BlockStop` シーケンスがあるか
- [ ] tool_call: `BlockStart` に `"block_type":"ToolUse"` を含むか
- [ ] long_text: `BlockDelta` が複数行あるか（ストリーミング分割の確認）
- [ ] 各 fixture に `Usage` イベントが含まれるか
- [ ] エラーイベントが混入していないか

## 注意事項

- fixture は API の応答に依存するため、モデルバージョンアップで再録画が必要になることがある
- 録画の自動化（CI での定期録画等）は行わない。手動実行 + 目視確認のフロー
- fixture が存在しない場合、対応するテストは skip される（`if !fixture_path.exists() { return; }` パターン）
