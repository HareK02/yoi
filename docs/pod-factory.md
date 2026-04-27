# Pod Factory: カスケード設定とプロンプト資産

`PodFactory` は、複数の層に分かれた `manifest.toml` とプログラマティック
overlay をマージして、検証済みの `PodManifest` と `PromptLoader` を生成する
ビルダー。これにより Pod 起動ごとに TOML を手書きする必要がなくなる。

---

## カスケード層

優先順位が低い順（上位ほど下位を上書き）:

| 優先度 | 層 | 位置 | 典型的な内容 |
|---|---|---|---|
| 1 | ビルトインのデフォルト | `manifest::defaults` モジュールの `pub const` 群を `PodManifestConfig::builtin_defaults()` が cascade 層として注入 | `tool_output.default_max_bytes = 16KB` など |
| 2 | ユーザー manifest | `<config_dir>/manifest.toml`（解決ルールは `manifest::paths`） | プロバイダ指定、デフォルトモデル、常用ツール設定 |
| 3 | プロジェクト manifest | 起動ディレクトリから上方向に探索した最初の `<root>/.insomnia/manifest.toml` | scope、compaction、プロジェクト固有の instruction |
| 4 | プログラマティック overlay | CLI / GUI / 別 Pod からの spawn 等 | `pod.name`、`pod.pwd` のような Pod 固有値 |

デフォルト値はすべて `crates/manifest/src/defaults.rs` の `pub const` として集約
されており、serde `#[default = "..."]` 経路（`PodManifest` の直接 deserialize）
と `TryFrom<PodManifestConfig>` 経路（cascade 解決）の両方が同じ constants を
参照する。デフォルトを変更するときは `defaults.rs` の 1 行を書き換えるだけで
全経路に反映される。

どの層も TOML スキーマは `PodManifest` と同じ（全フィールド省略可）。

## マージセマンティクス

| フィールド種別 | 規則 |
|---|---|
| スカラー（`String`, `u32`, `bool` 等） | 上層に値があれば丸ごと置換 |
| `Option<T>` | 上層が `Some` なら置換、`None` なら据え置き |
| マップ（`tool_output.per_tool` 等） | キー単位でマージ、同一キーは上層優先 |
| `scope.allow` / `scope.deny` | **union**（各層から全部足す）。上位層は `deny` で下位層の `allow` を必ず削れる |

各層をマージした結果（`PodManifestConfig`）を `TryFrom<PodManifestConfig>
for PodManifest` が必須フィールド検証と絶対パス検証をかけて `PodManifest`
に変換する。

## パス解決

manifest 中のパス（`provider.api_key_file` / `scope.*.target` /
`compaction.provider.api_key_file`）は相対記述を許容する。相対パスは
**各層のベース基準**で層ごとに絶対化され、そのあとで cascade merge に
かかる。層をまたいだ相対の意味ブレ（user 層の `./keys` が project 層の
どこを指すのか曖昧）を避けるための設計。

| 層 | ベース |
|---|---|
| user manifest (`<config_dir>/manifest.toml`) | そのファイルの親ディレクトリ |
| project manifest (`<project>/.insomnia/manifest.toml`) | **プロジェクトルート**（`.insomnia/` の親）。`target = "."` がワークスペース全体を指すように |
| overlay（inline TOML・programmatic） | プロセスの `current_dir()` |

Pod の作業ディレクトリは manifest に含まれない。プロセス起動時の
`std::env::current_dir()` がそのまま Pod の pwd となるため、別の作業
ディレクトリで Pod を走らせたい場合は `cd` してから `pod` を起動する
（または `SpawnPod` が子に対して行っているように、親プロセス側で
`Command::current_dir` を明示する）。

cascade merge 後の `TryFrom<PodManifestConfig>` では `ensure_absolute`
が不変条件チェックとしてだけ働く。相対パスが残っていれば上流の
resolve 段を取りこぼしている証拠なので `ResolveError::RelativePath` を
返す。

## 未知フィールドと型エラー

- **未知フィールド**: `tracing::warn!` を出して無視。将来バージョンアップで読めない
  旧設定が出るとユーザー体験が悪いため、`#[serde(deny_unknown_fields)]` は使わない。
- **型ミスマッチ**: `max_tokens = "100"` のような型エラーは hard error として
  resolve 失敗させる。ファイルパスと位置情報をエラーメッセージに含める。

---

## manifest.toml 例

### ユーザー層（最小）

`<config_dir>/manifest.toml`:

```toml
[model]
ref = "anthropic/claude-sonnet-4-6"
auth = { kind = "api_key", file = "/home/you/.config/insomnia/keys/anthropic" }
```

`ref = "<provider>/<model_id>"` はプロバイダ / モデルカタログを引く短縮形。
`scheme` / `base_url` / `model_id` は provider 側の宣言から引かれ、`auth` も
カタログの `auth_hint` を起点に解決する。ここでは env 既定（`INSOMNIA_API_KEY_ANTHROPIC`）
ではなく file から読みたいので `auth` だけ override している。詳細は
`crates/provider/README.md` と `resources/{providers,models}/builtin.toml` を参照。

### プロジェクト層（最小）

`<project>/.insomnia/manifest.toml`:

```toml
[[scope.allow]]
target = "/abs/path/to/project"
permission = "write"

[[scope.deny]]
target = "/abs/path/to/project/secrets"
permission = "read"

[compaction]
compact_threshold = 80000
```

### 全オプション例

```toml
[pod]
name = "reviewer"

# Form A: ref のみ（カタログから scheme / base_url / auth_hint / capability を全部引く）
#   [model]
#   ref = "anthropic/claude-sonnet-4-6"
#
# Form B: ref + 部分 override（ここで示している形）
#   カタログ起点に個別フィールドだけ上書き。ref 指定時は scheme / model_id / auth は任意 override。
#
# Form C: 完全 inline（カタログ無視。実験用 / カタログに無いモデル）
#   [model]
#   scheme = "anthropic"
#   model_id = "claude-sonnet-4-6"
#   auth = { kind = "api_key", file = "..." }
[model]
ref = "anthropic/claude-sonnet-4-6"
base_url = "https://api.anthropic.com"
auth = { kind = "api_key", file = "/home/you/.config/insomnia/keys/anthropic" }

[worker]
instruction = "$user/reviewer"
max_tokens = 4096
max_turns = 50
temperature = 0.3
reasoning = "medium"  # 文字列 = effort label / 整数 = thinking budget tokens。詳細は docs/reasoning.md

[worker.tool_output]
default_max_bytes = 16384

[worker.tool_output.per_tool]
Read = 32768
Grep = 4096

[[scope.allow]]
target = "/abs/path/to/project"
permission = "write"

[[scope.allow]]
target = "/abs/path/to/docs"
permission = "read"
recursive = false

[[scope.deny]]
target = "/abs/path/to/project/secrets"
permission = "write"

[compaction]
prune_protected_turns = 3
prune_min_savings = 4096
compact_threshold = 80000
compact_retained_turns = 2

[compaction.provider]
kind = "gemini"
model = "gemini-2.0-flash"
api_key_file = "/home/you/.config/insomnia/keys/gemini"
```

---

## instruction とプロンプト資産

### `worker.instruction` フィールド

Pod のシステムプロンプトの**本体**として使うプロンプト資産への参照。
import-map 形式のプレフィックスで指定する:

| プレフィックス | 解決先 |
|---|---|
| `$insomnia` | バイナリ同梱の `resources/prompts/`（`include_dir!`） |
| `$user` | `<config_dir>/prompts/`（`manifest::paths` で解決） |
| `$workspace` | `<project>/.insomnia/prompts/` |

- `.md` 拡張子は省略する（例: `$insomnia/default` → `resources/prompts/default.md`）
- 省略時のデフォルト値は `$insomnia/default`（`defaults::DEFAULT_INSTRUCTION`）
- 指定した prefix の dir に該当ファイルが無ければ **hard error**（fallthrough しない）

### ビルトインプロンプト

`resources/prompts/` 以下に同梱:

| 名前 | 用途 |
|---|---|
| `default` | デフォルトの instruction 本体。workspace / tool-usage をインクルード |
| `common/workspace` | cwd・日付の注入 |
| `common/tool-usage` | ツール使用の共通ガイダンス |

### `{% include %}` の相対解決

テンプレート内で `{% include "name" %}` のようにプレフィックス無しで書いた場合、
**include を書いたファイル自身のプレフィックスとディレクトリ**からの相対で解決する:

- `$insomnia/default.md` 内の `{% include "common/workspace" %}` → `$insomnia/common/workspace`
- `$user/custom.md` 内の `{% include "$insomnia/common/tool-usage" %}` → 明示的プレフィックスが優先

### システムプロンプトの最終構造

`instruction` テンプレートのレンダリング結果に、Rust 側で以下の**固定セクション**を付加する。
ユーザーテンプレートからは触れない領域:

```
<instruction のレンダ結果>

---
## Working boundaries

<scope.summary()>

---                            ← AGENTS.md が不在なら省略
## Project instructions (AGENTS.md)

<AGENTS.md 本文>               ← AGENTS.md が不在なら省略
```

- scope セクションは**必ず**出力される
- AGENTS.md セクションは不在時に区切り `---` ごと省略

---

## `pod` CLI

```
pod [--user-manifest <path>] [--project <path>] [--overlay <toml>]
    [-s/--store <path>]
```

| フラグ | 説明 |
|---|---|
| `--user-manifest` | ユーザー manifest のパス。省略時は `manifest::paths::user_manifest_path()` で自動解決 |
| `--project` | プロジェクト manifest 探索の起点。省略時は cwd から上方向に `.insomnia/` を探索 |
| `--overlay` | 最上層の overlay を inline TOML 文字列で渡す（例: `--overlay 'worker.instruction = "$user/foo"'`） |
| `-s, --store` | セッション永続化ディレクトリ（デフォルト: `<data_dir>/sessions/`、`manifest::paths` で解決） |

Pod の作業ディレクトリは `pod` 起動時の cwd が直接使われる。別ディレクトリで
動かしたい場合は `cd <path> && pod ...` のように外側で `cd` してから起動する。

引数無しで起動すると、cwd + `manifest::paths` の自動解決だけで動く最小構成になる
（overlay 無し、プロジェクトに `.insomnia/manifest.toml` があればそれを使う）。

---

## プログラマティック API

```rust
use pod::{Pod, PodFactory};

let (manifest, loader) = PodFactory::new()
    .with_user_manifest_auto()?      // manifest::paths から自動読み込み、不在 OK
    .with_project_manifest_auto()?   // cwd から上方向に .insomnia/ を探索、不在 OK
    .with_overlay_toml(overlay)?     // programmatic な最上層 overlay
    .resolve()?;                     // -> (PodManifest, PromptLoader)

let pod = Pod::from_manifest(manifest, store, loader).await?;
```

`Pod::from_manifest_toml(toml, store)` は単層 manifest を TOML 文字列で直接投げる
便利関数（テスト・デバッグ向け）。builtins-only のプロンプトローダで動く。
