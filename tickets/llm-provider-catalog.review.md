# Review: LLM プロバイダ/モデルカタログ

## 前提・要件の確認

### 要件 1: TOML 形式の定義（`[[provider]]` 配列、`auth_hint = { kind, env? }`）
満たされている。
- `crates/provider/assets/providers.toml` の 4 エントリがチケット例（`tickets/llm-provider-catalog.md:15-46`）と一字一句合致している（id / display_name / scheme / base_url / auth_hint.kind / env 名 / default_models の順序と値）。
- `AuthHint` 側は `#[serde(tag = "kind", rename_all = "snake_case")]` でインラインテーブル `{ kind = "api_key", env = "..." }` を受け付ける（`crates/provider/src/catalog.rs:44-56`）。`kind = "codex_oauth"` はデフォルトの `snake_case` 変換が `codex_o_auth` になってしまう問題を `#[serde(rename = "codex_oauth")]` で明示的に回避しており、既存 `AuthRef::CodexOAuth`（`crates/manifest/src/model.rs:70`）と同じ書き方になっている。整合している。

### 要件 2: `Vec<ProviderEntry>` を返す読取 API を `crates/provider` に 1 本公開
満たされている。
- `pub fn load() -> Result<Vec<ProviderEntry>, CatalogError>` が主 API（`catalog.rs:108`）。補助として `load_builtin` / `load_from_path` も公開されており、後者はテストと将来の明示パス指定に合理的。
- 配置は `crates/provider` 直下（`lib.rs:14` で `pub mod catalog;`）。`llm-worker` に下りていない。Memory の「llm-worker は低レベル基盤に留める」方針に整合。

### 要件 3: 配置パス（builtin の `include_str!` / user override 置換 / 両方読めなければ builtin）
満たされている。
- `BUILTIN_CATALOG: &str = include_str!("../assets/providers.toml")`（`catalog.rs:18`）。
- user override は `$XDG_CONFIG_HOME/insomnia/providers.toml`、未設定時は `$HOME/.config/insomnia/providers.toml`（`catalog.rs:137-154`）。チケットでは `$HOME` フォールバックは明示されていないが、XDG Base Directory spec の既定挙動として妥当で、過剰とも言えない。
- マージではなく置換（`catalog.rs:108-115`）。`load_prefers_override_over_builtin` / `load_falls_back_to_builtin_when_override_absent` テストで両経路をカバー。

### 要件 4: `ProviderEntry` + `model_id` → `ModelConfig` の変換
満たされている。
- `ProviderEntry::to_model_config(&self, model_id: impl Into<String>) -> ModelConfig`（`catalog.rs:82-99`）が `AuthHint` の 3 バリアントを `AuthRef` に 1:1 で写す。`capability` は常に `None` で返し、`build_client` 側の 3 段階 fallback（`ModelConfig.capability` → `capability::lookup` → `scheme default`、`lib.rs:120-124`）に委ねるのはチケット設計判断「`capability` 宣言をカタログに入れない」（`llm-provider-catalog.md:77-79`）と整合。
- `to_model_config_maps_auth_hint` テスト（`catalog.rs:208-229`）で 3 バリアント全部を確認。

### 要件 5: 完了時の動作（builtin 取得 / override 置換 / UI 未実装段階で unit test e2e）
満たされている。
- `ollama_entry_builds_client`（`catalog.rs:231-239`）が「カタログ読取 → `ProviderEntry` 選択 → `ModelConfig` 生成 → `build_client` が成功」の経路を端から端まで通している。Ollama は `AuthRef::None` なので環境変数依存がなく、常に成功する選択として適切。
- `cargo test -p provider` が 43 件全部 pass、ワークスペース `cargo build` もクリーン。

## アーキテクチャ・スコープ

- **レイヤ分離**: `crates/provider` 内に閉じ、`llm-worker` には漏れていない。`build_client` / `resolve_auth` の既存シグネチャは一切変更していない（`lib.rs:53-146` は今回の差分外）。非破壊性 OK。
- **責務分離**: `AuthHint`（UI メタ）と `AuthRef`（ランタイム解決）を別型として定義し、`to_model_config` で 1:1 変換する設計は、チケット「設計判断: `auth_hint` と `AuthRef` の二重定義」（`llm-provider-catalog.md:73-75`）の意図を正確に反映している。型を流用する代替案もあり得たが、UI ヒントに `file: Option<PathBuf>` が紛れ込むとカタログのスコープが広がるため、別型にしたのは妥当。
- **過剰抽象の有無**: `discover: Option<DiscoverMode>` のような先取り実装はされていない（コメントで言及のみ、`catalog.rs:60-61`）。チケットの「auto_discover は別チケット」方針に従っている。
- **依存追加**: `toml` を dev-dependencies から regular dependencies に「移動」する形で `cargo add` 相当の編集がされている。手動編集ではあるが、既存行を move しただけなので Memory の `Use cargo add` 方針からの逸脱は軽微。
- **モジュール分割**: `pod.rs` 等の既存 primary ファイルに詰め込まず、`catalog.rs` として独立モジュール化している。Memory の「feature モジュール分割」方針に合致。
- **Scope 外の侵食**: UI / auto_discover / 編集 UI / プロファイルのどれにも触れていない。境界遵守。

## 指摘事項

### Non-blocking / Follow-up
- `$HOME` フォールバック（`catalog.rs:143-152`）がチケット本文に明示されていない。妥当な挙動ではあるが、チケットの「配置パス」記述と実装の解像度に乖離があり、ユーザーが後から読んだとき驚きがある。ticket 側に一行追記するか、コード側のドキュメント（モジュール冒頭 doc）に「XDG 既定挙動として `$HOME/.config/...` にフォールバック」を明記しておくと親切。

### Nits
- `catalog.rs:106` の doc コメントに "sigh" というタイポがある（"silent" の意図と思われる）。該当箇所:

  ```
  /// 存在すれば builtin を置き換える。存在しなければ builtin のみ。
  /// user override が存在するが壊れている場合はエラーを返す（silent
  /// fallback はしない — ユーザーが書いた設定が sigh なく無視されて
  /// builtin に戻る挙動は気付きにくいため）。
  ```

  `sigh` → `silent` に直すか、日本語化で「気付かれず」等に差し替え。
- `builtin_ollama_shape`（`catalog.rs:173-183`）の assert が `scheme == Anthropic` を確認しているが、コメントの「Ollama = Anthropic scheme」の意図は `lib.rs:278-290` の `ollama_succeeds_without_key` と重複する情報。コメントで相互参照しておくと将来の読み手に親切。

## 判断

**Approve（完了可）** — チケット要件 1〜5 は全て明確に満たされており、`AuthHint`/`AuthRef` の責務分離・レイヤ境界・既存 API 非破壊といったアーキテクチャ観点も適切。指摘は `sigh` タイポと `$HOME` フォールバックのドキュメント明示の 2 点のみで、どちらも blocking ではない。
