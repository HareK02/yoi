# LLM モデル設定の再編 — レビュー

## 前提・要件の再確認

`docs/plan/llm_providers.md` の第一級/二次/非サポート方針と、チケット本体の 9 要件 + 6 設計決定 を前提に、実装が意図と整合しているかを確認した。変更量は +1608 / -1174 行、41 ファイル。

`cargo check` および `cargo test --workspace --lib` は通過。

## 要件達成度

| # | 要件 | 状況 | メモ |
|---|---|---|---|
| 1 | Pod マニフェスト `[model]` 宣言 | ✓ | `manifest/src/model.rs` + `config.rs::ModelConfigPartial` |
| 2 | `providers/` 層廃止 | ✓ | 4 ファイル削除、`HttpTransport<S>` 1 本に集約 |
| 3 | 既存 scheme 再編（openai → openai_chat、Ollama は流用） | ✓ | 各 scheme に `scheme_impl.rs` + `capability.rs` 追加 |
| 4 | `AuthRef` 分離 | ✓ | `manifest/model.rs` + `llm_client/auth.rs::AuthRequirement` |
| 5 | 第一級/二次 方針整合 | ✓ | `provider/lib.rs::build_client` + `SchemeKind` |
| 6 | `ModelCapability` 分離 | △ | 型定義・静的テーブル・default はあり。**`ModelConfig` からのマニフェスト override は未実装** |
| 7 | Streaming 現状維持 | ✓ | Event 型変更なし、`Scheme::State` で Anthropic block_type 補完 |
| 8 | Ollama 運用注意点 | ✓ | `default_capability` で Anthropic scheme が `CacheStrategy::Auto`、`ollama_succeeds_without_key` テストあり |
| 9 | 完了時動作 | ✓ | ビルド通過、既存テスト通過 |

## 設計決定の反映

| # | 決定 | 反映 |
|---|---|---|
| 1 | `Scheme` trait 方針A（全面抽象化） | ✓ `scheme/mod.rs::Scheme` に URL/認証/ヘッダ/body/SSE を集約。`State` associated type で Anthropic の `block_type` 補完を綺麗に処理 |
| 2 | `AuthRef` 組合せ検証 方針B（構築時） | ✓ `ResolvedAuth::matches` + `build_transport` で照合 |
| 3 | `crates/provider` 方針A（残す） | ✓ 薄いファクトリとして維持 |
| 4 | `ModelCapability` ハイブリッド | △ scheme 側静的テーブルはあるが、マニフェスト override 側が欠落 |
| 5 | フィールド単位 override | ✓ `ModelConfigPartial::merge` |
| 6 | TOML 後方互換切り | ✓ 旧 `[provider]` は完全に新 `[model]` に置換 |

## 指摘事項

### 優先度: 中

#### 1. 要件 6 の「ModelConfig で明示宣言すれば override」が未実装

`ModelConfig` 構造体に `capability: Option<ModelCapability>` フィールドがない。設計決定 4（ハイブリッド: scheme 側テーブル → マニフェスト override → デフォルト）の **override 側が欠落**している。

影響:
- scheme 静的テーブルに無いモデル（OpenRouter / xAI の Grok / Groq の Kimi / OpenAI 互換ルーター系）は必ず `default_capability(scheme)` に落ちる
- 二次サポートの共通枠の実用性に直結（例: Grok の `ReasoningSupport::Effort` が効かず reasoning 送れない）

対応案:
- A. 今チケットで `ModelConfig.capability: Option<ModelCapability>` を追加し `build_transport` で優先順位 `config.capability → scheme.capability_for → default_capability` に
- B. Scope 外として明示し別チケットに切り出す

ユーザーの決定方針（二次サポートを共通枠でカバー）からすると A が自然。ticket 本体の Scope 外にも記載がないため、A を推奨。

#### 2. `validate_config` の機能退化

旧 `providers/openai.rs` の `OpenAIClient` は `LlmClient::validate_config` をオーバーライドし「OpenAI は `top_k` 非対応」の warning を出していた。削除された `tests/validation_test.rs` はこの warning をテストしていた。

今の `HttpTransport<S: Scheme>` は `validate_config` をオーバーライドしておらず、`LlmClient` trait のデフォルト実装（空 `Vec`）が使われる。つまり **`Worker::validate()` は scheme による制約（top_k, logprobs 等）を検出できなくなっている**。

対応案:
- `Scheme` trait に `validate_config(&RequestConfig) -> Vec<ConfigWarning>` を追加し、`HttpTransport` 側で scheme に委譲
- 最低限 OpenAI Chat scheme で旧と同じ top_k warning を再実装
- 合わせて新形式での regression test を追加（旧 `validation_test.rs` の代替）

### 優先度: 低

#### 3. `default_capability` の配置

`crates/provider/lib.rs::default_capability` が `SchemeKind` ごとに直書き（同じ情報が `Scheme` trait 実装側と分離して存在）。`Scheme::default_capability()` メソッドに移動する方が関心事が集約される。今の形でも動作するが、新 scheme 追加時に 2 箇所編集が必要な点が弱い。

#### 4. `AuthRequirement` 判定の緩さ

`ResolvedAuth::matches` は `(None, _) => true` で常にパス。これは Ollama Anthropic 流用（`AuthRef::None` で `XApiKey` 要求）のための意図的設計だが、本来認証必須の scheme（Anthropic 本家）に誤って `AuthRef::None` を渡しても構築成功し、実行時の 401 で初めて失敗する。

より厳密にするなら `AuthRequirement::XApiKeyOptional` のようなバリアント導入で分離できるが、実害は小さいので現状維持も許容範囲。

#### 5. rustdoc のクロス crate リンク

`scheme/mod.rs:47` の `[AuthRef](../../../manifest/enum.AuthRef.html)` は相対リンクで、`cargo doc --workspace` 時に切れる可能性。`[`manifest::AuthRef`]` 形式のクロス crate リンクにしておくと rustdoc が解決できる。

## アーキテクチャ評価

### 良い点
- `Scheme::State` associated type の導入で「Anthropic の `content_block_stop` に `block_type` が載らない」といった具体的な痛みを抽象内で解決
- `ResolvedAuth::matches` による構築時検証が `build_transport` 1 箇所に集約、分岐が明瞭
- `ModelConfigPartial::merge` のフィールド単位 override が既存の cascade layer と自然に噛み合う
- Ollama 運用の制約（`cache_control`/`tool_choice`/`metadata` 不可）が capability + scheme 側送出制御で分散、`provider::tests::ollama_succeeds_without_key` で境界条件がテストされている
- `spawn_pod.rs::overlay_inherits_spawner_model` テストで親 Pod の `ModelConfig` が子にシームレスに伝播することを確認

### コードベースを歪めていないか
- 旧 `providers/` の 4 ファイルは綺麗に削除、重複は残っていない
- `SchemeKind::OpenaiResponses` はマニフェスト側に先行存在するが、`build_client` で `SchemeNotImplemented` エラーを明示的に返す（別チケットで肉付け前提）。これは依存チケット設計通り
- `AuthRef::CodexOAuth` も同様に予約のみ、`resolve_auth` でエラーを返す

### 不必要な実装
- `AuthRequirement::Custom` バリアントは Codex OAuth 用の先行予約で、今チケットでは使われない。将来の拡張点として小さい負債、許容範囲

## 総合判定

**コア要件は達成されている。構造再編は綺麗で、Scope の切り方も妥当**。実装ミス的な重大欠陥はなく、既存テストも全て通過している。

ただし以下 2 点の判断が必要:

1. **要件 6 の `ModelConfig.capability` override**: 今チケットで追加するか、Scope 外として明示するか
2. **`validate_config` の退化**: 復活させるか、ticket 本体に「OpenAI の top_k warning 等の validate 機能は scheme 再編で意図的に落とした」旨を明記するか

どちらも「今 close して後で別チケット」でも進められるが、最低限ticket 本体への記載（Scope 外明示 or 後続タスク言及）が必要。
