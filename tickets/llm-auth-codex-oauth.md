# Codex OAuth 認証の流用

**Status: Reviewed / Approved**（詳細は `llm-auth-codex-oauth.review.md`）

## 背景

決定済み方針（`docs/plan/llm_providers.md`）で、ChatGPT サブスクリプションの OAuth トークンを流用して OpenAI Responses API を叩く経路を第一級サポートとする。OpenAI は Codex CLI を Apache-2.0 で公開し、ChatGPT OAuth の第三者ツール利用を service terms で名指し禁止していない（互換経路）。

Codex CLI の実装（github.com/openai/codex、`codex-rs/login/` 配下）から以下が確定:

- トークンは `~/.codex/auth.json` に `{ auth_mode, tokens: { id_token (JWT 文字列), access_token (JWT), refresh_token, account_id }, last_refresh }` 形式で保存。`OPENAI_API_KEY` フィールドも同居する場合あり
- リクエストは `https://chatgpt.com/backend-api/v1/responses` に投げる（Responses API wire）
- 認証ヘッダは `Authorization: Bearer <access_token>` + `ChatGPT-Account-Id: <account_id>`（FedRAMP 組織なら `X-OpenAI-Fedramp: true`）
- リフレッシュは `https://auth.openai.com/oauth/token` に `{ client_id: "app_EMoamEEZ73f0CkXaXp7hrann", grant_type: "refresh_token", refresh_token }` を JSON POST。response は `{ id_token?, access_token?, refresh_token? }`、401 body の `error.code` が `refresh_token_expired|reused|invalidated` で永続失敗を分類
- proactive refresh の判定: access_token JWT の `exp` claim が `now` 以下、fallback で `last_refresh < now - 8 days`（バッファなし）
- Codex CLI 自身はファイルロックを取らず、(a) プロセス内 `AsyncMutex` (b) refresh 前の guarded reload (account_id 比較で他プロセスの先行更新を検知) (c) 書込前の再 load + diff merge で並行動作を吸収
- Codex CLI の credentials store はデフォルト `File` (auth.json)。`Keyring` / `Auto` モードは opt-in

## 要件

1. **`AuthRef::CodexOAuth` の追加**: `manifest::AuthRef` に ChatGPT OAuth バリアントを追加（型定義済み、provider 側で実装する）

2. **トークン読み取り**: `~/.codex/auth.json` を読み、以下を取り出す
   - `tokens.access_token` / `tokens.refresh_token` / `tokens.account_id`
   - `last_refresh`（fallback 期限判定用）
   - `tokens.id_token` の JWT payload を base64url decode し `https://api.openai.com/auth` claim から `chatgpt_account_is_fedramp` (bool) と `chatgpt_plan_type` を取得（署名検証なし）

3. **ヘッダ注入**: `HttpTransport` が `AuthRef::CodexOAuth` を解決するとき以下を組み立てる
   - `Authorization: Bearer <access_token>`
   - `ChatGPT-Account-Id: <account_id>`
   - FedRAMP 組織なら `X-OpenAI-Fedramp: true`

   実装方針: llm-worker 側で `ResolvedAuth::Custom(Arc<dyn AuthProvider>)` バリアントと `AuthProvider` trait（async でリクエスト毎にヘッダを返す）を新設し、`HttpTransport` の `AuthRequirement::Custom` 経路から呼ぶ。Codex 固有ロジック（auth.json 読取・refresh）は `crates/provider/src/codex_oauth/` に置く。

4. **base_url の自動適用**: `AuthRef::CodexOAuth` 指定時、`ModelConfig.base_url` 未指定なら `https://chatgpt.com/backend-api` を既定とする。明示指定（API key 経路と共用したい場合の `https://api.openai.com` 等）は尊重。`scheme` 側の既定（`https://api.openai.com`）は変えず、`build_client` で auth に応じて差し替える。

5. **トークンリフレッシュ**:
   - 送信前に proactive チェック: access_token JWT の `exp` claim が `now` 以下、fallback で `last_refresh + 8 days < now`。バッファは持たせない（Codex 準拠）。401 駆動の retry は将来拡張
   - refresh は `https://auth.openai.com/oauth/token` に上記 body を POST、response の `access_token` / `id_token` / `refresh_token` を auth.json に書き戻し `last_refresh = now` を更新
   - 並行動作の排他: プロセス内 `tokio::sync::Mutex` を取った上で、refresh 直前に再 load し account_id が一致しないなら自分はスキップして読み直した値を採用（guarded reload）。書込時も再 load + diff merge（Codex CLI に揃える、ファイルロックは使わない）
   - 失敗時: `RefreshTokenError::Permanent`（401 + `refresh_token_expired|reused|invalidated`）は `ClientError::Auth` 相当で「`codex login` を再実行してください」のメッセージ、`Transient` (network 等) はそのまま伝播

6. **scheme/openai_responses との組合せで動作**: `ModelConfig { scheme: OpenAIResponses, base_url: (既定), model_id: "gpt-5-codex" 等, auth: CodexOAuth }` で ChatGPT 枠を使って Codex 相当の動作ができる

7. **完了時の動作**: ChatGPT アカウント保持者が `codex login`（File モード）済みの環境で insomnia を起動すると、追加設定なしで Codex と同じモデル（`gpt-5-codex` 等）が利用可能

## 設計判断

### auth.json の書き戻しと競合制御

ファイルロックは取らない。Codex CLI 自身も取っていない（単純な truncate write + `mode 0o600`）。代わりに guarded reload + 書込前 merge で吸収する。`fs2` 依存を増やさない方が筋が良い。

### refresh token の失効時の挙動

`Permanent` 分類のエラーをそのまま `ClientError::Auth { message }` で返し、CLI/TUI 上で「`codex login` を再実行」のメッセージを表示する。insomnia 側で再ログインフローは持たない。

### ChatGPT OAuth 専用モデル (gpt-5-codex 等)

`scheme/openai_responses/capability.rs` の静的テーブルに該当モデル ID を追加するのみ。API key 経路で使われたら OpenAI 側が 401 で弾くので動的フォールバックは不要。

### Keyring モード対応

Scope 外。Codex CLI の `cli_auth_credentials_store = "keyring"` で保存しているユーザーには `~/.codex/config.toml` を `"file"` に切り替えて `codex login` をやり直すよう案内する（auth.json 不在エラー時にメッセージで誘導）。

### セキュリティと権限

`~/.codex/auth.json` のパーミッション（600）を尊重。書き込み時に `OpenOptions` で `mode(0o600)` を再設定する（Codex CLI と同じ）。

## Scope 外

- Claude Pro/Max OAuth 経路（方針上非採用）
- `claude -p` CLI fork 経路
- `codex login` 自体の実装（Codex CLI に任せ、insomnia は auth.json を読むのみ）
- Codex CLI の Keyring credentials store 対応
- 401 駆動の動的 refresh + retry（proactive のみで実装）
- ChatGPT backend の rate limit 観測（Retry-After 処理は HttpTransport 共通の責務）

## 依存

- `manifest::AuthRef::CodexOAuth`（型のみ定義済み）
- `llm_worker::llm_client::transport::HttpTransport` / `auth::AuthRequirement::Custom`（経路の枠は用意済み）
- `scheme/openai_responses`（`/v1/responses` wire format、実装済み）
