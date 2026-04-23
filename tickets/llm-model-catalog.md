# LLM モデルカタログ + マニフェスト ref 解決

## 背景

`llm-provider-catalog` で provider 列挙の宣言化までは入った（`crates/provider/assets/providers.toml`、各エントリは `default_models: Vec<String>`）。しかし:

- **モデルの capability は依然 `crates/provider/src/capability.rs` の prefix matching ハードコード**で解決している（`model_id.starts_with("claude-")` 等）。`llm-capability-ownership` でハードコードを「scheme 個別判定 → provider 層の prefix 判定」に粗めただけで、宣言的 resource にはなっていない。
- **マニフェストは毎回 scheme / model_id / auth を直書き**している。`scheme` は本来 model_id から派生する属性で、マニフェストが知る必要のない wire 知識。
- provider カタログの `default_models[]` は ID 文字列だけで、capability・display_name 等のモデル単位メタが乗らない。

この 3 つを「モデルカタログを resource 化し、マニフェストはそこから ref で引く」に揃えると、`capability.rs` のハードコードが丸ごと不要になり、マニフェストが「`anthropic/claude-sonnet-4-6` を使う」レベルまで簡潔になる。

## 要件

### 1. リソース配置

```
resources/
  providers/builtin.toml    ← 既存 crates/provider/assets/providers.toml を移動
  models/builtin.toml       ← 新設
```

両方 `include_dir!` で同梱（`crates/pod/src/prompt_loader.rs:27` の `resources/prompts` と同パターン）。user override は引き続き `$XDG_CONFIG_HOME/insomnia/{providers,models}.toml`、置換採用（マージしない）。

### 2. provider カタログ形状の変更

```toml
[[provider]]
id = "anthropic"
display_name = "Anthropic"
scheme = "anthropic"
base_url = "https://api.anthropic.com"
auth_hint = { kind = "api_key", env = "INSOMNIA_API_KEY_ANTHROPIC" }
default_capability = { tool_calling = "parallel", structured_output = "json_schema", reasoning = "budget_tokens", vision = true, prompt_caching = { explicit = { max_breakpoints = 4 } } }
```

- `default_models[]` を削除（モデルカタログが真の情報源）
- `default_capability` を追加（モデルカタログ未登録時のフォールバック先）
- 他フィールドは現行どおり

### 3. モデルカタログ

```toml
[[model]]
id = "claude-sonnet-4-6"
provider = "anthropic"
capability = { reasoning = "budget_tokens", vision = true, ... }

[[model]]
id = "claude-opus-4-1"
provider = "anthropic"
# capability 省略可: provider.default_capability にフォールバック

[[model]]
id = "anthropic/claude-sonnet-4"
provider = "openrouter"

[[model]]
id = "openai/gpt-5"
provider = "openrouter"

[[model]]
id = "gpt-5-codex"
provider = "codex-oauth"
capability = { reasoning = "effort", ... }
```

- `id` は **provider 内ユニーク**。同じ `gpt-5` が異なる provider に存在するのは OK
- `provider` は providers カタログの `id` を参照。解決時 hard error 検証
- `capability` 省略時は provider.default_capability にフォールバック

### 4. マニフェストの 2 形式（untagged）

```rust
// crates/manifest/src/model.rs を置き換え
struct ModelManifest {
    r#ref:      Option<String>,           // "<provider>/<model_id>"
    scheme:     Option<SchemeKind>,
    model_id:   Option<String>,
    base_url:   Option<String>,
    auth:       Option<AuthRef>,
    capability: Option<ModelCapability>,
}
```

3 つのユースケース:

```toml
# Form A: ref のみ（カタログから全部引く）
[model]
ref = "anthropic/claude-sonnet-4-6"

# Form B: ref + override（カタログ起点に部分上書き）
[model]
ref = "anthropic/claude-sonnet-4-6"
auth = { kind = "api_key", file = "./sk-ant.local" }

# Form C: 完全直書き（カタログ無視。実験用 / カタログに無いモデル）
[model]
scheme = "anthropic"
model_id = "claude-sonnet-4-6"
auth = { kind = "api_key", file = "./sk-ant.local" }
```

Validation:
- `ref` 無し → `scheme` / `model_id` / `auth` の 3 つが必須
- `ref` あり → 他フィールドはすべて任意 override

### 5. 解決ロジック

`ref` を first `/` で split → `(provider_id, model_id_in_ref)`（OpenRouter の `openrouter/anthropic/claude-sonnet-4` も provider=`openrouter` / model_id=`anthropic/claude-sonnet-4` で通る）。

| ステップ | 失敗時 |
|---|---|
| provider 引き | **hard error**（マニフェストロード失敗） |
| model 引き | **warn ログ + provider.default_capability にフォールバック**（API 側で model_id 拒否されたら自然に runtime エラー） |

最終 `ModelConfig` の各フィールド:

| フィールド | 解決順 |
|---|---|
| `scheme` | manifest 明示 > provider.scheme |
| `base_url` | manifest 明示 > provider.base_url |
| `model_id` | manifest 明示 > ref の `<model_id>` 部分 |
| `auth` | manifest 明示 > provider.auth_hint 由来の `AuthRef` |
| `capability` | manifest 明示 > model catalog の capability > provider.default_capability > **`Scheme::default_capability`**（ref 無しで capability 省略時の最終セーフティネット） |

### 6. 削除

- `crates/provider/src/capability.rs` 丸ごと削除（prefix matching ハードコード）
- `crates/provider/assets/providers.toml` 削除（resources/ に移動するため）
- provider カタログの `default_models[]` フィールド削除

### 7. 完了時の動作

- `[model] ref = "anthropic/claude-sonnet-4-6"` だけ書いた最小マニフェストで Pod が起動できる
- ref 無しの完全直書きマニフェスト（既存 test_pod.*.local.toml 形式）も依然動く
- 既存 `test_pod.anthropic.local.toml` / `test_pod.codex.local.toml` を ref 形式に書き換え、いずれの形式でも build_client が成功する unit / integration test
- builtin の 4 provider と各 default_models 相当のモデルがモデルカタログに登録済み

## 設計判断

### prefix matching を消す根拠

`capability::lookup` は「`claude-` で始まれば全部同じ capability」「`gpt-5` 以降と `o1/o3/o4` は Reasoning family」と判定している。これは:
- 新モデルが出るたびに prefix が当たるかコードで検証する必要がある（`gemini-3` を入れる行が `capability.rs:141` に手書きで足された等）
- OpenRouter 経由の `anthropic/claude-sonnet-4` は prefix が `anthropic/` で当たらない（router 越しのモデルが構造的にカバー外）
- `claude-sonnet-4-6` も `claude-haiku-4-5` も同じ capability にされる（モデル単位の差異が消える）

モデル単位で宣言する resource があれば、これらは全部「カタログにエントリを足すだけ」で解決する。

### `ref` 形式を採用しても直書きを残す理由

カタログに無いモデル（出たての新モデル、ローカル LLM の任意 ID、社内モデル等）を試すときに「カタログに足してから動かす」を強制すると実験コストが上がる。直書き形式は escape hatch として残す。capability も省略可にして scheme default に落とす（wire-level 安全側で動く、推奨ではない）。

### model_id の重複

provider 内ユニーク、provider 跨ぎは許容。`openrouter` の `openai/gpt-5` と `codex-oauth` の `gpt-5` が両方カタログにあって構わない。ref が `<provider>/<model_id>` で必ず provider を含むため曖昧性が出ない。

### model catalog 未登録モデルでも動く扱い

`ref = "anthropic/some-new-model"` のように **provider はカタログにあるが model は無い** ケースは、warn ログを出して provider.default_capability にフォールバック。「タイポで動いてしまった」を見逃さないよう warn は出すが、新モデルを試す際の摩擦を増やさないため hard error にはしない。本当に存在しないモデル ID なら API 側で `model not found` 系のエラーが返るので、誤魔化しにはならない。

### user override の挙動

`llm-provider-catalog` で決めた「user override があれば builtin を置換、マージしない、壊れた TOML はエラー」をモデルカタログにもそのまま適用。providers と models は独立して読む（片方だけ user override も可）。

### capability 重複の回避

model catalog の capability と provider.default_capability の両方を持つと「どっちが効くか」が分かりにくいが、解決順は明確に「model 単位 > provider default」と一方向で、両方書いた場合は model 単位が勝つ。「provider レベルで spectrum を決め、モデル単位で必要なときだけ上書き」という運用を想定。

## Scope 外

- UI 実装（モデル選択画面・provider 選択画面）
- auto_discover（Ollama `/api/tags`、OpenRouter `/models` 動的列挙）
- カタログファイル編集 UI
- プロファイル（同一 provider で複数の API key を切り替える）
- 既存 `Scheme::default_capability` の見直し（残置・現行値そのまま）

## 依存

- `llm-provider-catalog` 完了済（本チケットで provider カタログ形状を変更し、`default_models` 削除 + `default_capability` 追加）
- `llm-model-config` 完了済（`ModelConfig` / `AuthRef` / `SchemeKind` / `ModelCapability`）
- `llm-scheme-openai-responses` / `llm-auth-codex-oauth` 完了済

## 影響範囲

- `resources/providers/builtin.toml` 新設（旧 `crates/provider/assets/providers.toml` を移動 + 形状変更）
- `resources/models/builtin.toml` 新設
- `crates/provider/src/catalog.rs`: provider 形状変更（`default_models` → `default_capability`）+ モデルカタログ追加 + ref 解決関数追加
- `crates/provider/src/capability.rs` 削除
- `crates/provider/src/lib.rs`: `capability::lookup` 経路を削除し、ref 解決関数経由に置換
- `crates/manifest/src/model.rs`: `ModelConfig` を `ModelManifest`（ref / inline 両受け）に置き換え。resolved 形の `ModelConfig` は `crates/provider` 側の内部型に
- `crates/pod/src/factory.rs` 等の manifest 経由で `ModelConfig` を組む箇所: `ModelManifest → ModelConfig` 解決関数を呼ぶ形に
- `test_pod.anthropic.local.toml` / `test_pod.codex.local.toml`: ref 形式に書き換え（インライン形式の動作確認も別途残す）
