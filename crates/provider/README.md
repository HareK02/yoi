# provider

マニフェストの `ModelManifest` から適切な `LlmClient`（`HttpTransport<S>`）を構築するファクトリクレート。プロバイダ / モデルカタログの解決、API キーの local secret store / 明示ファイル解決、scheme ↔ auth の整合検証を担う。

## 公開型

- `build_client(manifest: &ModelManifest) -> Result<Box<dyn LlmClient>, ProviderError>` — ref / inline を受け取り、カタログ解決 → `HttpTransport<S>` 構築までを行う
- `build_client_from_config(config: &ModelConfig) -> Result<Box<dyn LlmClient>, ProviderError>` — 解決済み `ModelConfig` から構築
- `catalog::resolve_model_manifest(&ModelManifest) -> Result<ModelConfig, ResolveError>` — ref / inline を `ModelConfig` へ解決（build せずに参照のみ欲しいケース向け）
- `catalog::{load_providers, load_models}` — builtin + user override を解決したカタログ
- `ProviderError` / `CatalogError` / `catalog::ResolveError` — エラー種別

## 責務

- プロバイダ / モデルカタログの builtin (`resources/{providers,models}/builtin.toml`) と user override (`$XDG_CONFIG_HOME/yoi/{providers,models}.toml`) の解決
- `ModelManifest` の ref 形を `(provider, model_id)` に split し、`ModelConfig` へ展開
- `AuthRef::SecretRef` / `AuthRef::ApiKey` を `ResolvedAuth::ApiKey` に解決（通常は local secret store、低レベル manifest では明示ファイルも可）
- `AuthRef::None` / `AuthRef::CodexOAuth` の解決
- `Scheme::required_auth()` と `ResolvedAuth` の妥当性検証（非対応組合せは構築エラー）
- capability は manifest 明示 > model catalog > provider.default_capability > `Scheme::default_capability()` の順で解決
- context window は manifest 明示 > model catalog > provider.default_context_window > builtin fallback の順で解決し、inline model でも `context_window` で override できる
