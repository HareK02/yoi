# manifest

Pod の宣言的設定を TOML マニフェストとして定義・パースするクレート。プロバイダ設定、ワーカー設定、ディレクトリスコープ制約を記述できる。

## 公開型

- `PodManifest` — Pod 設定全体（`from_toml()` でパース）
- `PodMeta` — Pod メタデータ（名前）
- `ProviderConfig` — LLM プロバイダ設定（種別、モデル、APIキー環境変数、ベースURL）
- `ProviderKind` — プロバイダ種別（`Anthropic`, `Openai`, `Gemini`, `Ollama`）
- `WorkerManifest` — ワーカー設定（システムプロンプト、max_tokens、temperature）
- `ScopeConfig` — スコープ設定（ルートディレクトリ）
- `Scope` — ディレクトリスコープの実行時チェック（`contains()` でパス包含判定）
