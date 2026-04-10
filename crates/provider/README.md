# provider

マニフェストの設定から適切な LLM クライアントを構築するファクトリクレート。APIキーの環境変数解決を含む。

## 公開型

- `build_client(config: &ProviderConfig) -> Result<Box<dyn LlmClient>, ProviderError>` — プロバイダ設定に応じたクライアント生成（Anthropic, OpenAI, Gemini, Ollama）
- `ProviderError` — クライアント構築エラー
