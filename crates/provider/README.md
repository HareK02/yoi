# provider

マニフェストの `ModelConfig` から適切な `LlmClient`（`HttpTransport<S>`）を構築するファクトリクレート。APIキーの環境変数 / ファイル解決と scheme ↔ auth の整合検証を担う。

## 公開型

- `build_client(config: &ModelConfig) -> Result<Box<dyn LlmClient>, ProviderError>` — `SchemeKind` と `AuthRef` から `HttpTransport<S>` を構築
- `ProviderError` — クライアント構築エラー

## 責務

- `AuthRef::ApiKey` を `ResolvedAuth::ApiKey` に解決（env → file の優先順位）
- `AuthRef::None` を `ResolvedAuth::None` に変換
- `Scheme::required_auth()` と `ResolvedAuth` の妥当性検証（非対応組合せは構築エラー）
- 既知モデルは scheme の静的テーブル、未知モデルは scheme 既定の `ModelCapability` を採用
