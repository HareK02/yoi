# manifest

Pod の宣言的設定を TOML マニフェストとして定義・パースするクレート。モデル設定、ワーカー設定、ディレクトリスコープ制約を記述できる。

## 公開型

- `PodManifest` — Pod 設定全体（`from_toml()` でパース）
- `PodMeta` — Pod メタデータ（名前、pwd）
- `ModelConfig` — LLM モデル設定（scheme、base_url、model_id、auth）
- `SchemeKind` — wire scheme 種別（`Anthropic`, `OpenaiChat`, `OpenaiResponses`, `Gemini`）
- `AuthRef` — 認証参照（`None`, `ApiKey { env, file }`, `CodexOAuth`）
- `WorkerManifest` — ワーカー設定（システムプロンプト、生成設定、reasoning）
- `ScopeConfig` / `ScopeRule` / `Permission` — allow / deny の宣言的スコープ設定
- `Scope` — 実行時スコープ。`from_config(&ScopeConfig, pwd)` で構築し、`is_readable` / `is_writable` / `permission_at` で問い合わせる
