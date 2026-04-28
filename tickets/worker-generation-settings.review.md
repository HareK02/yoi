# Review: LLM 生成設定の manifest 露出整理

## 前提・要件の確認

- `[worker]` への `top_p` / `top_k` / `stop_sequences` 追加: 満たされている。
  - `WorkerManifestConfig` に `top_p: Option<f32>` / `top_k: Option<u32>` / `stop_sequences: Option<Vec<String>>` を追加 (`crates/manifest/src/config.rs:65-73`)。
  - `WorkerManifest` 側にも `top_p` / `top_k` / `stop_sequences: Vec<String>`（解決後は確定値、未指定は空 Vec）として露出 (`crates/manifest/src/lib.rs:104-109`)。
  - `TryFrom<PodManifestConfig>` で `cfg.worker.stop_sequences.unwrap_or_default()` により未指定時は空配列に正規化 (`crates/manifest/src/config.rs:351-353`)。
- cascade merge の置換意味論: 満たされている。`upper.top_p.or(self.top_p)` 等のスカラーと、`upper.stop_sequences.or(self.stop_sequences)` の配列もまるごと置換になっている (`crates/manifest/src/config.rs:235-237`)。要件「`stop_sequences` は配列の追記マージをしない」と一致。
- `apply_worker_manifest()` から `RequestConfig` への反映: 満たされている。`request_config_from_worker_manifest` ヘルパーに切り出し、`top_p` / `top_k` を `Some` のときだけ代入し、`stop_sequences` はクローンして代入 (`crates/pod/src/pod.rs:1395-1411`)。
- 未指定時の wire 互換: 満たされている。
  - `RequestConfig.stop_sequences: Vec<String>` (`crates/llm-worker/src/llm_client/types.rs:596`) は各 scheme 側で `#[serde(skip_serializing_if = "Vec::is_empty")]` 付きフィールドへ複製される（anthropic `request.rs:33-34`、gemini `request.rs:141-142`、openai_chat も同様）。空 Vec のとき body から欠落するため、新フィールド未指定時は今までと同じ wire になる。
  - `top_p` / `top_k` も `Option` のまま `skip_serializing_if = "Option::is_none"` 経路で従来通り省略される。
- parse / merge / apply の三段テスト: 揃っている。
  - parse: `crates/manifest/src/config.rs:773` `from_toml_accepts_worker_generation_settings` と `crates/manifest/src/lib.rs:316-358` の TOML 統合テスト。
  - merge: `crates/manifest/src/config.rs:601` `merge_worker_generation_settings_upper_wins`。`top_k` は upper=None で lower=Some(20) が残ること、`stop_sequences` は upper 配列が下位を完全に置換することを検証。
  - apply: `crates/pod/src/pod.rs:1630` `worker_manifest_generation_settings_become_request_config`。
- 既存テストの未指定アサーション: `crates/manifest/src/lib.rs:308-310` で `top_p.is_none() / top_k.is_none() / stop_sequences.is_empty()` を確認。defaults 経路の不変も明示されている。

## アーキテクチャ・スコープ

- 既存の cascade パターン（`Option<T>::or`）にそのまま乗っており、`stop_sequences` を `Option<Vec<String>>` で持って `unwrap_or_default()` で `Vec` に展開する実装は、他の `worker.*` フィールドと意味論を揃えるための妥当な選択。`Vec` 直持ちにすると「未指定」と「明示的な空配列」が区別できなくなり cascade で下位を上書きする意味が出せなくなるため、この選択は適切。
- `apply_worker_manifest` から `request_config_from_worker_manifest` を抽出した小さなリファクタは、`RequestConfig` ビルドが純粋関数になりテスト容易性が上がる範囲に収まっている。過剰な抽象化はなく YAGNI 違反なし。
- `llm-worker` の `RequestConfig` 自体には変更なし、既に存在するフィールドへ繋ぐだけで manifest 側を上の層から制御している。`feedback_llm_worker_scope` の方針（高レベル機能は上の層）に沿う。
- 範囲外項目（`reasoning` の manifest 露出、`presence_penalty` 等の新規パラメータ、provider 別値域検証）には踏み込んでいない。
- `cargo add` 経路の懸念なし（依存追加なし）。
- README (`crates/manifest/README.md`) のサマリも「生成設定、reasoning」と整合の取れた表現に更新されている。

## 指摘事項

### Blocking

- なし。

### Non-blocking / Follow-up

- docs の範囲外編集が混在している。`docs/architecture.md` の `pwd` 行削除、`docs/pod-factory.md` の overlay 例から `pod.pwd` を外す変更、`compaction.provider` → `compaction.model` への置換、旧 `compact_retained_turns` → `compact_request_threshold` / `compact_retained_tokens` / `compact_auto_read_budget` / `compact_worker_max_input_tokens` への差し替え、パス解決例の更新（`provider.api_key_file` → `model.auth.file`）は、既に main 側で実装が進んでいた仕様に docs を追従させる修正であって、本チケットのスコープではない。動作上は健全だが、別チケットとして括り出すか commit 粒度を分けると履歴の追跡性が上がる。今回はチケット完了の判断を妨げない。
- `docs/pod-factory.md` の新設「`[worker]` 設定」テーブルにある `top_k` 説明「未対応 scheme では warning または provider 側挙動に任せる」は、本チケットの範囲外の値域検証文言に踏み込み気味。実際 `validate_config` 経路で warning を出すかは scheme 実装に依存するため、軽い表現緩和（例: 「scheme が未対応の場合は scheme 側の投影に従う」）の検討余地あり。

### Nits

- `request_config_from_worker_manifest` 内の `if let Some(top_p) = wm.top_p { config.top_p = Some(top_p); }` は `config.top_p = wm.top_p;` でも等価。`max_tokens` / `temperature` の既存記法に合わせている意図と思われるので変更は必須でないが、`config.top_p = wm.top_p; config.top_k = wm.top_k;` のほうが意図（「未指定なら未指定のまま伝搬」）が読みやすい。
- `merge_worker_generation_settings_upper_wins` テストで `top_k` を upper 未指定にして lower の `Some(20)` が残ることを確認しているのは良い設計。ここに `stop_sequences` の「upper 未指定なら lower がそのまま残る」ケースが加わるとカバレッジが完全になる（現在は upper が指定されたケースのみ）。

## 判断

Approve — 要件・完了条件をいずれも満たし、cascade の置換意味論・未指定時の wire 互換・三段テストの整備すべて確認済み。docs の範囲外編集は履歴粒度の指摘に留まり、コードベースを歪める変更はない。
