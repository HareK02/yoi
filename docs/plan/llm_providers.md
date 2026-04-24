# LLM プロバイダサポート方針

## Context

INSOMNIA が利用する LLM プロバイダとその認証方式を決める。従量 API 課金の心理的負担を避け、定額サブスク枠（ChatGPT Codex / Ollama Cloud）を活かせる構成にする。

詳細な現状調査は `docs/ref/llm-provider-landscape.md` と `docs/ref/llm-pricing-2026-04.md` を参照。

## 決定事項

### 第一級サポート（専用アダプタ）

| プロバイダ | scheme | 認証 | 用途 |
|---|---|---|---|
| **Ollama** | scheme/anthropic 流用（v0.14+ `/v1/messages`） | なし（ダミー） | ローカル + `:cloud` サフィックスでクラウド中継。`localhost:11434` で統一 |
| **Codex OAuth 経路** | scheme/openai_responses | `~/.codex/auth.json` | ChatGPT の定額枠を利用 |
| **Anthropic API** | scheme/anthropic | API key | 従量課金経路のみ |

Ollama は独自 scheme を作らず `scheme/anthropic` を base_url 差し替えで流用。`/v1/chat/completions` は stream+tools バグ (#9092) のため使わない。`cache_control` / `tool_choice` / `metadata` / `count_tokens` は Ollama 非対応のため送らない。

### 二次サポート（OpenAI 互換共通枠）

`scheme = "openai_chat"` + `base_url` + API key を宣言するプロバイダカタログエントリ（`resources/providers/builtin.toml`）1 つで以下を収容。個別モデルはモデルカタログ（`resources/models/builtin.toml`）側で列挙する:

- OpenRouter
- xAI (Grok)
- Groq
- Together / Fireworks / DeepInfra
- BLACKBOX
- 任意の OpenAI 互換エンドポイント

### 非サポート

- **Claude Pro/Max OAuth 経路** — Anthropic が 2026-01-09 にサーバ側でブロック、2026-02-19 に ToS で第三者ツール経由を明文禁止。リスクが第一級機能に見合わない
- **`claude -p` CLI fork** — 同様に採用しない。実装しない

## 根拠

- **Codex OAuth は Codex CLI 互換**: Codex CLI は Apache-2.0、openai/codex #8338 で OpenAI 社員が fork 自由と明言、service terms に名指し禁止なし
- **Anthropic API は従量だが代替なし**: Pro/Max OAuth 封鎖後、Claude 系を使うには API key 経路のみ
- **Ollama は `:cloud` で透過**: `ollama signin` で Ed25519 鍵登録後、`localhost:11434` 経由でクラウドモデルが使える。ローカルデーモンが署名付き中継
- **OpenAI 互換は汎用アダプタ 1 本**: ルーター系は後追いで数を増やしやすい宣言型設計、実装コスト最小

## 実装原則

- 認証ストアを読むアダプタ（`~/.codex/auth.json` 等）は **llm-worker 直下に置かず上位層に配置**。llm-worker は低レベル基盤に留める方針（`feedback_llm_worker_scope.md`）と整合
- モデル列挙は **auto_discover と宣言型の両輪**。Ollama は `/api/tags` で自動、OpenAI 互換枠はモデルカタログ（`resources/models/builtin.toml` + `$XDG_CONFIG_HOME/insomnia/models.toml` の user override）で宣言
- UI のプロバイダ選択肢も第一級 → 二次の優先順位で並べる
- **`ollama launch insomnia` 対応を視野に**、env 注入（`ANTHROPIC_BASE_URL` / `OPENAI_BASE_URL` 等）で起動設定を受け入れる作り

## 機能方針

### プロバイダ側の高次ツールは使用しない

`web_search` / `code_interpreter` / `computer_use` / Live Search 等はプロバイダ依存を避けるため不採用。insomnia の自前 Tool 層で統一。fallback や routing もワーカー側で管理するため OpenRouter の `transforms` / `provider routing` も使わない。

### 必須 capability

- **tool calling**: parallel tool calls 含めて必須
- **reasoning**: 必須。内部表現は共通型 `ReasoningControl { effort, budget_tokens }` に正規化し、scheme アダプタで各社形式（`reasoning.effort` / `thinking.budget_tokens` / `reasoning_format`）に投影。DeepSeek の `reasoning_content` 別フィールドは Thinking block として正規化

### Capability は 5 軸

`ModelCapability` として以下を持つ:

1. tool calling（parallel 可否含む）
2. structured output (`json_object` / `json_schema` / Grammar)
3. reasoning（effort / budget_tokens / 出力形式）
4. vision
5. prompt caching（下記）

### Prompt caching は 2 値

```
prompt_caching: CacheStrategy,   // Explicit { max_breakpoints } / Auto
```

- **Explicit**: Anthropic。scheme が `cache_control` マーカー挿入
- **Auto**: それ以外全部。scheme は何もしない（サーバ側自動 prefix と「何もしない」は呼び出し側から同一）
- 「安定 prefix を先頭に寄せる」正規化は両方共通で適用

### Streaming

現状の `BlockStart / BlockDelta / BlockStop / BlockAbort` + `DeltaContent::{Text, Thinking, InputJson}` 設計を維持。変更不要。

ToolCall 引数が delta で来ないプロバイダ（Gemini）は scheme アダプタで「BlockStart → InputJson(全体 1 回) → BlockStop」の**擬似ストリーム化**で吸収。

Ollama は Ollama 側の OpenAI 互換エンドポイント (`/v1/chat/completions`) に stream+tool バグ (#9092) があるため、**Anthropic Messages API 互換エンドポイント (`/v1/messages`) に寄せる** (`scheme/anthropic` を `base_url` 差し替えで流用、`cache_control` / `tool_choice` は送らない)。

## Scope 外

- 各アダプタのクレート構成・データ型・API 境界は別 plan/ticket で切り出す
- プロバイダ選択 UI とモデル一覧管理の設計も別扱い
- OpenAI Responses API の stateful (`previous_response_id`) 対応は将来対応、`BlockMetadata` の拡張が必要
