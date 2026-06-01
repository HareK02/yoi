# LLM プロバイダ統合の外部事例

調査日: 2026-04-19。プロバイダ認証経路・`ollama launch` 等の時事的項目は陳腐化が早い。数値・URLは一次ソースで再確認すること。

## 各ハーネスのプロバイダ対応方式

### Zed
- ネイティブ: Anthropic / OpenAI / Google / Ollama / DeepSeek / Mistral / OpenRouter / Vercel AI Gateway
- `openai_compatible` スロットで任意プロバイダ追加
- OpenRouter は**宣言型**: `available_models[]` に `max_tokens` / `supports_tools` 等の capability を書く。自動 discovery ではない
- Ollama は `auto_discover: bool` でローカル tag 自動列挙
- https://zed.dev/docs/ai/llm-providers

### OpenCode (sst/opencode)
- Vercel AI SDK + Models.dev で 75+ プロバイダ
- 認証は `~/.local/share/opencode/auth.json` に統一保存（OAuth / APIキー / その他の3種別）
- **2026-03-19 に Anthropic OAuth 対応を削除**（PR #18186）。詳細は後述
- ChatGPT ブラウザ認証 (`/connect`) は存続
- https://opencode.ai/docs/providers/

### OpenClaw
- Peter Steinberger らの個人向け self-hosted AI（OpenCode と別物）
- 30+ プロバイダ対応
- Anthropic 独自 OAuth 実装は縮小、現在は `claude -p` (Claude Code CLI) の subprocess 再利用を推奨
- OpenRouter は「OpenAI 互換プロキシ」扱いで固有機能 (serviceTier, prompt-cache hints) は転送しない
- https://docs.openclaw.ai/concepts/model-providers

### Aider / Cline / Roo / Continue.dev
- Aider は LiteLLM 経由、他は直叩き
- BLACKBOX 等マイナー系は OpenAI 互換枠で収容するのが一般パターン

## 認証経路の現状

### Anthropic (Claude Pro / Max) ── 封鎖済み
- 2026-01-09: Anthropic がサーバ側で Pro/Max OAuth トークンに `This credential is only authorized for use with Claude Code and cannot be used for other API requests` の制限導入
- 2026-02-19: Claude Code の Legal & Compliance ページに "Authentication and credential use" セクション追加。「OAuth 認証は Claude Code 等の通常利用のため専用」「Agent SDK を含む第三者製ツールで Free/Pro/Max 資格情報を経由するのは許可しない」と明記
- 2026-03-19: OpenCode が PR #18186 で以下を削除
  - `packages/opencode/src/session/prompt/anthropic-20250930.txt`（Claude Code 風システムプロンプト）
  - `opencode-anthropic-auth@0.0.13` ビルトインプラグイン
  - `claude-code-20250219` beta ヘッダ
- 代替検討: `claude -p` (Claude Code の headless mode) を subprocess で呼ぶ方式。ACP ではなく素朴な CLI fork であり、yoi では採用しない
- https://code.claude.com/docs/en/legal-and-compliance
- https://github.com/sst/opencode/pull/18186

### OpenAI (Codex CLI / Responses)
- Codex CLI は Apache-2.0 で公開されている。yoi の Codex OAuth 経路は、Codex CLI と同じ Responses 系 wire behavior に寄せる
- Codex CLI の認証ストアと conversation header / request compression / SSE behavior を参考にする
- OpenCode の `/connect` で ChatGPT ブラウザ認証が通る
- コミュニティ評価: 「Anthropic は walled garden、OpenAI はむしろ取り込みに来た」
- https://github.com/openai/codex/discussions/8338
- https://developers.openai.com/codex/auth

### CLI fork 方式 (`claude -p`)
- `claude --print` / `claude -p` は Claude Code の非対話（headless）モード。プロンプトを stdin/引数で受け stdout に返す
- **ACP ではなく素朴な subprocess 呼び出し**
- OpenClaw と OpenCode コミュニティフォーク (`griffinmartin/opencode-claude-auth`) が採用
- yoi では専用 API integration ではないため採用しない

## Ollama の統合機構

### `ollama launch <tool>`
- v0.14.0 (2026-01) 以降、Ollama サーバ本体に **Anthropic Messages API 互換レイヤー**が組み込まれた
- 別プロセスのプロキシは起動しない。Ollama コア (`localhost:11434`) が `/v1/messages` 相当を喋る（streaming / system prompt / tool calling / extended thinking / vision 対応）
- `ollama launch claude` はサブコマンドバイナリを fork し、子プロセスに env を注入
  - `ANTHROPIC_BASE_URL=http://localhost:11434`
  - `ANTHROPIC_AUTH_TOKEN=ollama`
  - `ANTHROPIC_API_KEY=`（空）
- 設定ファイル書き換えやシェル rc 操作はしない
- `ANTHROPIC_BASE_URL` はハードコードで `OLLAMA_HOST` を見ない（issue #13936）
- サブコマンド `claude` / `codex` / `opencode` / `droid` / `clawdbot` は「ツール毎の env 注入テンプレート」の集合体
  - 例: `opencode` は `OPENCODE_CONFIG_CONTENT` に JSON を流し込み OpenCode 側で deep-merge
  - `codex` は OpenAI 互換 (`/v1/chat/completions`) を向けるので Anthropic 系ではなく OpenAI 系 env を設定
- https://ollama.com/blog/launch
- https://ollama.com/blog/claude
- https://github.com/ollama/ollama/issues/13936

### Ollama クラウド
- `ollama signin` は OAuth ではなく **Ed25519 鍵ペア登録**。`~/.ollama/id_ed25519`（秘密鍵）と `.pub`（公開鍵）をローカル保存、ブラウザで `ollama.com/connect` から公開鍵をアカウントに紐付け
- ルーティング: `server/routes.go` の `GenerateHandler` / `ChatHandler` がモデル名をパースし、`:cloud` サフィックスなら `server/cloud_proxy.go` 経由で `ollama.com:443` に中継
- 署名: タイムスタンプ付きチャレンジを Ed25519 秘密鍵で署名し `Authorization` ヘッダに載せる
- レスポンスは 32KB バッファで chunk 中継
- **クライアントは `localhost:11434` のままでよい**。ローカル Ollama デーモンが透過プロキシ役を果たす
- `ollama.com/api` を直叩きしたいときのみ `OLLAMA_API_KEY` を別途発行
- https://github.com/ollama/ollama/blob/main/server/cloud_proxy.go

## OpenAI 互換プロバイダ事例

「共通 OpenAI 互換枠」で収容する代表的プロバイダ。`base_url` + API key 差し替えで動く。

### xAI (Grok)
- `base_url`: `https://api.x.ai/v1`（OpenAI SDK 互換、Anthropic SDK 互換レイヤーも提供）
- 主要モデル:
  - `grok-code-fast-1` — 256K ctx、$0.20/$1.50 /1M tok、SWE-bench 70.8%、142 tok/s（Sonnet の 1/10 以下の単価）
  - `grok-4-1-fast` — 2M ctx、$0.20/$0.50 /1M tok（長文向け差別化）
  - `grok-4` — 256K ctx、$3.00/$15.00 /1M tok
- プロンプトキャッシュあり（50–75% 引）、Batch API 50% 引
- 認証は API key のみ、OAuth なし
- サブスク（X Premium+ / SuperGrok / SuperGrok Heavy）は UI 専用で **API 枠を含まない**
- Zed / OpenCode / Cline / Cursor に第一級サポートあり
- https://docs.x.ai/developers/models

### Groq
- `base_url`: `https://api.groq.com/openai/v1`（OpenAI SDK 互換）
- 特徴: LPU で TTFT <100ms、モデル別 TPS 500–1000（`gpt-oss:120b` で 500 tok/s 等）
- 主要モデル:
  - GPT-OSS 120B — $0.15/$0.60 /1M tok、128K ctx
  - Kimi K2-0905 — $1.00/$3.00、256K ctx（open モデル最強クラス）
  - Llama 3.3 70B Versatile — $0.59/$0.79、131K ctx
  - Qwen3 32B — $0.29/$0.59、131K ctx
- tool_use / JSON mode / streaming すべて OpenAI 互換
- プロンプトキャッシュ 50% 引、Batch API 50% 引
- **Qwen3-Coder-480B は未提供**。Coder 用途は Together / Cerebras 等で補完する前提
- Cerebras 比較: GPT-OSS 120B で Cerebras ~3000 tok/s vs Groq ~476 tok/s。throughput は Cerebras、TTFT は Groq
- 月額サブスクは Enterprise のみ、通常は従量
- https://groq.com/pricing
- https://console.groq.com/docs/api-reference

### BLACKBOX AI
- `base_url`: `https://cloud.blackbox.ai/api` (OpenAI 互換 REST)、Bearer `bb_*` トークン
- 主要ハーネスに専用サポートなし。共通枠で登録するのが実用
- Unlimited プランは FUP 非公開で実態不明
- https://docs.blackbox.ai/api-reference/authentication

### OpenRouter
- `base_url`: `https://openrouter.ai/api/v1`
- 多数のプロバイダを単一 API で束ねる（料金は passthrough + プリペイド手数料 5.5%）
- BYOK: 月 100 万 req 無料、超過で元コストの 5%
- `:free` モデル 50 req/日（$10 入金で 1000 req/日）
- https://openrouter.ai/docs

## プロバイダ独自拡張

各プロバイダが OpenAI 互換エンドポイントに載せている独自機能。詳細は `llm-pricing-2026-04.md` と合わせて参照。

### OpenAI 本家
- `/v1/responses` stateful API、`previous_response_id`、`reasoning.effort` (minimal/low/medium/high/xhigh)、`reasoning.summary: "auto"`、`store=false` / ZDR 組織で強制
- 高次ツール: `web_search` / `code_interpreter` / `computer_use`
- `max_tokens` → `max_completion_tokens` (o系) → `max_output_tokens` (Responses) のリネーム

### xAI (Grok)
- OpenAI + Anthropic 両 SDK 互換（**Anthropic 互換は deprecated**、native 推奨）
- `reasoning_effort: low/high` は Grok-3 mini 系のみ、Grok-4 系は常時 reasoning
- **Deferred Chat Completions** で `request_id` による非同期取得
- Live Search

### Groq
- `service_tier: on_demand / flex / auto`（flex は 10x rate、失敗許容、paid 限定）
- **prompt caching は自動・追加料金なし・コード変更なし**
- `reasoning_format` で reasoning の提示方式制御（JSON 出力との両立）

### DeepSeek
- `deepseek-reasoner` はレスポンスに **`reasoning_content` 別フィールド**
- **reasoner では function calling 非対応**
- 入力に `reasoning_content` を含めると 400
- 自動 prefix cache (64 tok 単位、hit で 90% 割引)

### Cerebras / Together AI
- JSON schema / tool calling / multi-turn tool calling / streaming + structured を全モデル対応

### Fireworks
- **Grammar mode (BNF)** が独自、任意出力形式を制約可能

### OpenRouter
- `provider` でルーティング/フォールバック、`transforms` で自動中抜き
- **`reasoning` 統一インターフェース**で各社差を吸収

### Ollama
- `/v1/chat/completions` は **"experimental"** 明記
- **stream + tools で単一ブロック返却のバグ** (#9092)
- context size は Modelfile の `num_ctx` で固定（API 側で設定不可）
- native `/api/chat` の方が機能フル
- v0.14+ で `/v1/messages` (Anthropic 互換) も追加

### Azure OpenAI
- URL が `{resource}/openai/deployments/{name}/chat/completions?api-version=...`、**model_id ではなく deployment 名**
- content filter が `finish_reason: content_filter` や 400 で観測

## Capability 軸

モデル/プロバイダごとの機能差を表現する軸。**プロバイダ側高次ツール (web_search / code_interpreter / computer_use / Live Search) は yoi では使用しない方針**のため capability 軸から除外。

### 1. tool calling
parallel tool calls 可否、tool_choice 対応度。DeepSeek reasoner のような「reasoner + tool 非対応」ケースあり。

### 2. structured output
- `json_object` のみ
- `json_schema` 対応
- Grammar (Fireworks 独自)

### 3. reasoning
- `reasoning.effort` (OpenAI / xAI)
- `thinking.budget_tokens` (Anthropic)
- `reasoning_content` 別フィールド出力 (DeepSeek)
- `reasoning_format` (Groq)
- OpenRouter は `reasoning` に統一投影

### 4. vision
モデル依存度が大

### 5. prompt caching
呼び出し側ロジックから見て 2 値に集約:
- **Explicit**: `cache_control` マーカー挿入（Anthropic のみ）
- **Auto**: scheme はマーカー挿入しない（OpenAI / DeepSeek / Groq の自動 prefix も、サーバ側で何もしないケースも同じ扱い）

### 付随パラメータ
- `max_tokens` 名の差（`max_tokens` / `max_completion_tokens` / `max_output_tokens`）、Anthropic は必須
- stateful (`previous_response_id`) は OpenAI Responses のみ
- streaming SSE の delta 粒度・usage 到達タイミング差

## ToolCall streaming の差異

| プロバイダ | ToolCall 引数の streaming |
|---|---|
| Anthropic | `input_json_delta` partial streaming — 安定 |
| OpenAI (chat) | `tool_calls[i].function.arguments` partial — 安定、parallel 時は複数 index 並走 |
| OpenAI Responses | reasoning item が別構造（`summary[]` / `encrypted_content`） |
| xAI / Groq / DeepSeek | OpenAI 互換、chat と同じ |
| Gemini | function_call は **delta なし、一括で返る** |
| Ollama `/v1` | **stream + tools で delta が欠けるバグ** (#9092) |
| Ollama `/api/chat` | native API、安定 |

→ Gemini / Ollama `/v1` は scheme アダプタで「BlockStart → InputJson(全体 1 回) → BlockStop」の**擬似ストリーム化**で共通化可能。

## yoi での採用方針

### 第一級サポート（専用アダプタ）
- **Ollama API** — ローカル + `:cloud` サフィックスで透過的にクラウド中継。エンドポイントは `localhost:11434` で統一
- **Codex OAuth** — `~/.codex/auth.json` を読み、Codex CLI 互換の Responses 経路として扱う。conversation header / compression / SSE behavior は公開実装に合わせる
- **Anthropic API** — 従量 API key 経路のみ

### 二次サポート（共通 OpenAI 互換枠）
- `provider_kind: openai_compatible` + `base_url` + `available_models[]` の共通スロットで OpenRouter / xAI / Groq / Together / Fireworks / DeepInfra / BLACKBOX 等を一括収容
- ルーター系は後追いで追加しやすい宣言型設計

### 非サポート
- **Claude Pro/Max OAuth 経路** — 2026-01-09 サーバ側ブロック、2026-02-19 に第三者ツール経由の利用制限を明文化。第一級機能としては採用しない
- `claude -p` CLI fork も専用 API integration ではないため実装しない

### 実装原則
- 認証アダプタ（外部 CLI の認証ストアを読む類）は llm-worker 直下ではなく上位アダプタ層に配置。llm-worker は低レベル基盤に留める原則（project memory）と整合
- モデル列挙は `auto_discover` と宣言型の両輪。Ollama は自動、ルーター系は宣言
- `ollama launch yoi` 対応を視野に入れ、env 注入 (`ANTHROPIC_BASE_URL` / `OPENAI_BASE_URL` 等) で起動設定を受け入れる作り
