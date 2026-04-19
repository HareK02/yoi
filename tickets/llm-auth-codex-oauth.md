# Codex OAuth 認証の流用

## 背景

決定済み方針（`docs/plan/llm_providers.md`）で、ChatGPT サブスクリプションの OAuth トークンを流用して OpenAI Responses API を叩く経路を第一級サポートとする。OpenAI は Codex CLI を Apache-2.0 で公開し、ChatGPT OAuth の第三者ツール利用を service terms で名指し禁止していない（互換経路）。

Codex CLI の実装（github.com/openai/codex、`codex-rs/login/` 配下）から以下が確定:

- トークンは `~/.codex/auth.json` に `{ auth_mode, tokens: { id_token, access_token, refresh_token, account_id }, last_refresh }` 形式で保存
- リクエストは `https://chatgpt.com/backend-api/v1/responses` に投げる（Responses API wire）
- 認証ヘッダは `Authorization: Bearer <access_token>` + `ChatGPT-Account-ID: <account_id>`（+ FedRAMP 組織で `X-OpenAI-Fedramp: true`）
- リフレッシュは `https://auth.openai.com/oauth/token` に `grant_type=refresh_token`, `client_id=app_EMoamEEZ73f0CkXaXp7hrann`, `refresh_token` を POST

## 要件

1. **`AuthRef::CodexOAuth` の追加**: `llm-model-config` で定義する認証列挙型に ChatGPT OAuth バリアントを追加

2. **トークン読み取り**: `~/.codex/auth.json` を読み、以下を取り出す
   - `tokens.access_token`
   - `tokens.refresh_token`
   - `tokens.account_id`
   - `last_refresh`（期限判定用）
   - `tokens.id_token` の JWT claims（`chatgpt_account_is_fedramp`、`chatgpt_plan_type` 参照）

3. **ヘッダ注入**: `HttpTransport` が `AuthRef::CodexOAuth` を解決するとき以下を組み立てる
   - `Authorization: Bearer <access_token>`
   - `ChatGPT-Account-ID: <account_id>`
   - FedRAMP 組織なら `X-OpenAI-Fedramp: true`

4. **base_url の自動適用**: `ModelConfig.base_url` 未指定時は `https://chatgpt.com/backend-api` を既定とする。ユーザーが明示的に `https://api.openai.com` を指定した場合は尊重（API key 経路と共用したいケース用）

5. **トークンリフレッシュ**:
   - `https://auth.openai.com/oauth/token` に `{ client_id: "app_EMoamEEZ73f0CkXaXp7hrann", grant_type: "refresh_token", refresh_token }` を POST
   - 有効期限が近い（access_token が期限切れ or 残り時間が閾値以下）とき自動更新
   - 更新後の token を `~/.codex/auth.json` に書き戻す
   - 複数プロセス（Codex CLI との並行実行等）を想定した排他制御

6. **scheme/openai_responses との組合せで動作**: `ModelConfig { scheme: OpenAIResponses, base_url: (既定), model_id: "gpt-5-codex" 等, auth: CodexOAuth }` で ChatGPT 枠を使って Codex 相当の動作ができる

7. **完了時の動作**: ChatGPT アカウント保持者が `codex login` 済みの環境で insomnia を起動すると、追加設定なしで Codex と同じモデル（`gpt-5-codex` 等）が利用可能

## 設計課題

### 1. auth.json の書き戻しと競合制御

`~/.codex/auth.json` は Codex CLI と共用。両方が並行動作する場面で refresh token が古い値で上書きされると再認証が必要になる。ファイルロックを取るか、更新前に再読み込みして最新値で書くか。

### 2. refresh token の失効時の挙動

refresh token も失効するケース（Anthropic のサーバ側遮断のようにサブスク側が塞ぐケース）で、ユーザーにどう通知するか。insomnia CLI で `codex login` の再実行を促すメッセージを返す。

### 3. ChatGPT OAuth 専用モデル

`gpt-5-codex` / `codex-mini-latest` 等、ChatGPT backend でのみ利用可能なモデル ID がある場合、API key 経路と区別が必要。`ModelCapability` で「CodexOAuth でのみ有効」フラグを持つか、API 側の 401 を受けて動的にフォールバックするか。

### 4. セキュリティと権限

`~/.codex/auth.json` のパーミッション（600）を尊重。読み取り時にパーミッション確認 + 書き込み時にパーミッション再設定。

## Scope 外

- Claude Pro/Max OAuth 経路（方針上非採用）
- `claude -p` CLI fork 経路
- `codex login` 自体の実装（Codex CLI に任せ、insomnia は auth.json を読むのみ）
- ChatGPT backend の rate limit 観測（Retry-After 処理は HttpTransport 共通の責務）

## 依存

- `tickets/llm-model-config.md`（`AuthRef` / `HttpTransport` 構造）
- `tickets/llm-scheme-openai-responses.md`（`/v1/responses` wire format）
