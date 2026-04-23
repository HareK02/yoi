# LLM プロバイダ/モデルカタログ

## 背景

llm-worker の基盤（`scheme` / `ModelCapability` / `AuthRef`）と `crates/provider` の構築ファクトリは `llm-model-config` / `llm-scheme-openai-responses` / `llm-auth-codex-oauth` 完了で揃った。Pod マニフェストは `ModelConfig { scheme, base_url, model_id, auth, capability }` を 1 個宣言する形で機能する。

一方で UI 層（`native-gui-mvp` / `tui-pod-spawn-ui`）から「使えるプロバイダと代表的なモデル ID を一覧で見せる」ために参照できる候補リストが無い。現状 UI はマニフェストを手書き前提で、プロバイダ追加がコード変更でなく宣言で済むという `docs/plan/llm_providers.md` の狙い（「ルーター系は後追いで数を増やしやすい宣言型設計」）が UI 側に届いていない。

`docs/plan/llm_providers.md` 本文に書かれていた `available_models[]` 相当は、llm-worker / manifest の基盤に載せるより、このカタログレイヤで宣言するのが素直。基盤は「1 個のモデルを動かす」責務に留め、「選択肢を提供する」責務を分離する。

## 要件

1. **カタログファイル形式の定義**: TOML 宣言で以下を表現できる

   ```toml
   [[provider]]
   id = "anthropic"
   display_name = "Anthropic"
   scheme = "anthropic"
   base_url = "https://api.anthropic.com"
   auth_hint = { kind = "api_key", env = "INSOMNIA_API_KEY_ANTHROPIC" }
   default_models = ["claude-sonnet-4-5", "claude-opus-4-1"]

   [[provider]]
   id = "ollama-local"
   display_name = "Ollama (local)"
   scheme = "anthropic"
   base_url = "http://localhost:11434"
   auth_hint = { kind = "none" }
   default_models = ["llama3.1", "qwen2.5-coder"]

   [[provider]]
   id = "codex-oauth"
   display_name = "ChatGPT (Codex OAuth)"
   scheme = "openai_responses"
   auth_hint = { kind = "codex_oauth" }
   default_models = ["gpt-5-codex", "gpt-5"]

   [[provider]]
   id = "openrouter"
   display_name = "OpenRouter"
   scheme = "openai_chat"
   base_url = "https://openrouter.ai/api/v1"
   auth_hint = { kind = "api_key", env = "INSOMNIA_API_KEY_OPENROUTER" }
   default_models = ["anthropic/claude-sonnet-4", "openai/gpt-5"]
   ```

   - `id` はカタログ内ユニーク。UI での選択・保存用
   - `default_models[]` は代表的な ID。UI はこれを候補として出すが、ユーザーが自由入力もできる想定
   - `auth_hint` は UI 向けメタ（env 名 / 認証不要 / OAuth 利用可 の表示に使う）。実際の認証解決は従来通り `crates/provider` が `AuthRef` から行う
   - `capability` はここでは宣言しない（scheme の静的テーブルと `ModelConfig.capability` の既存 2 段階で足りる）

2. **読取 API**: カタログを読み `Vec<ProviderEntry>` として返す関数を 1 本公開する。配置先は `crates/provider`（認証解決と同じ層、plan の「llm-worker は低レベル基盤に留める」原則と整合）。

3. **配置パス**:
   - builtin: `crates/provider/assets/providers.toml` を `include_str!` で同梱（代表的な 4 経路 = Anthropic / Ollama / Codex OAuth / OpenRouter を最低限入れる）
   - user override: `$XDG_CONFIG_HOME/insomnia/providers.toml` があれば **マージではなく置換** で優先採用（マージ規則の複雑さは避ける）
   - 両方読めない場合は builtin のみ

4. **ProviderEntry → ModelConfig の変換**: UI が「このプロバイダのこのモデルを使う」を選んだときに、`ProviderEntry` + `model_id` から `ModelConfig` を組める変換関数を提供する。`auth_hint.kind` が `AuthRef` の各バリアントに対応する。

5. **完了時の動作**:
   - `crates/provider` の公開 API から builtin プロバイダ一覧が取得できる
   - user override ファイルが置かれていれば置換採用される
   - UI 未実装段階でも、unit test で「カタログ読取 → `ProviderEntry` 選択 → `ModelConfig` 生成 → `build_client` が成功する」経路が通る

## 設計判断

### builtin と user override の関係

マージ（builtin に追記する）方式は競合ルール（`id` 衝突時の優先・部分上書き）が必要になり、UI が「どの設定が効いているか」を説明しづらくなる。user override があれば完全置換とし、ユーザーは builtin をコピーして編集するか、自分で最小構成を書くかを選ぶ。

### `auth_hint` と `AuthRef` の二重定義

一見冗長だが、`auth_hint` は「UI が何を表示・要求するか」のヒント、`AuthRef` は「ランタイムがどこからトークンを引くか」の宣言で責務が違う。`auth_hint.kind = "api_key"` から `AuthRef::ApiKey { env: auth_hint.env, file: None }` への変換関数 1 本で繋ぐ。

### capability 宣言をカタログに入れない

scheme 側の静的テーブルに未登録のモデル ID（OpenRouter 経由の任意モデル等）を使うときは `ModelConfig.capability` で明示指定するという経路が既に存在する。カタログ側で更に capability を持つと 3 箇所で capability が定義できることになり、優先順位が混乱する。カタログは「選択肢の提示」に専念する。

### auto_discover は別チケット

Ollama `/api/tags` を叩いてモデル一覧を動的に取る機構は欲しいが、本チケットの静的カタログが動いてから別チケットで足す。`ProviderEntry` に後から `discover: Option<DiscoverMode>` を任意で足せる構造で設計しておく（フィールド拡張予定のコメントだけ残す）。

## Scope 外

- UI 実装（GUI / TUI 側のプロバイダ・モデル選択画面は各 UI チケットで）
- auto_discover（Ollama `/api/tags`、OpenRouter `/models` 等の動的列挙）
- カタログファイル編集 UI
- プロファイル（同一プロバイダで複数の API key を切り替える等）

## 依存

- `llm-model-config` 完了済（`ModelConfig` / `AuthRef` / `SchemeKind` / `ModelCapability`）
- `llm-scheme-openai-responses` 完了済（Responses scheme）
- `llm-auth-codex-oauth` 完了済（`AuthRef::CodexOAuth` 解決）

## 影響範囲

- `crates/provider/src/`: カタログ読取 + `ProviderEntry` 型 + 変換関数を追加
- `crates/provider/assets/providers.toml`: 新設（builtin カタログ）
- 他クレートに型は露出するが、既存 API に破壊的変更は入れない

## Review
- 状態: Approve
- レビュー詳細: [./llm-provider-catalog.review.md](./llm-provider-catalog.review.md)
- 日付: 2026-04-23
