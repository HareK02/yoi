# Pod Factory: カスケード設定とプロンプト資産

`PodFactory` は、複数の層に分かれた `manifest.toml` とプログラマティック
overlay をマージして、検証済みの `PodManifest` と `PromptLoader` を生成する
ビルダー。これにより Pod 起動ごとに TOML を手書きする必要がなくなる。

---

## カスケード層

優先順位が低い順（上位ほど下位を上書き）:

| 優先度 | 層 | 位置 | 典型的な内容 |
|---|---|---|---|
| 1 | ビルトインのデフォルト | `manifest::defaults` モジュールの `pub const` 群を `PodManifestConfig::builtin_defaults()` が cascade 層として注入 | `tool_output.default_max_bytes = 64 KiB`, `file_upload.max_bytes = 256 KiB` など |
| 2 | ユーザー manifest | `<config_dir>/manifest.toml`（解決ルールは `manifest::paths`） | プロバイダ指定、デフォルトモデル、常用ツール設定 |
| 3 | プロジェクト manifest | 起動ディレクトリから上方向に探索した最初の `<root>/.insomnia/manifest.toml` | scope、compaction、プロジェクト固有の instruction |
| 4 | プログラマティック overlay | CLI / GUI / 別 Pod からの spawn 等 | `pod.name`、spawn 時の `worker.instruction` のような Pod 固有値 |

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
| 配列スカラー（`worker.stop_sequences` 等） | 上層に値があれば配列ごと置換。追記マージはしない |
| マップ（`tool_output.per_tool` 等） | キー単位でマージ、同一キーは上層優先 |
| `scope.allow` / `scope.deny` | **union**（各層から全部足す）。上位層は `deny` で下位層の `allow` を必ず削れる |
| `permissions.rule` | **union**（下位層の rule → 上位層の rule の順に評価）。`permissions.default_action` は上位層があれば上書き |

各層をマージした結果（`PodManifestConfig`）を `TryFrom<PodManifestConfig>
for PodManifest` が必須フィールド検証と絶対パス検証をかけて `PodManifest`
に変換する。

## パス解決

manifest 中のパス（`model.auth.file` / `scope.*.target` /
`compaction.model.auth.file`）は相対記述を許容する。相対パスは
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
ディレクトリで Pod を走らせたい場合は `cd` してから `insomnia-pod` を起動する
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
top_p = 0.9
top_k = 40
stop_sequences = ["\n\n", "</stop>"]
reasoning = "medium"  # 文字列 = effort label / 整数 = thinking budget tokens。詳細は docs/reasoning.md

[worker.tool_output]
default_max_bytes = 65536

[worker.tool_output.per_tool]
Read = 131072
Grep = 8192

[worker.file_upload]
max_bytes = 262144

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

[permissions]
default_action = "allow" # allow | deny | ask

[[permissions.rule]]
tool = "Bash"
pattern = "rm *"
action = "deny"

[[permissions.rule]]
tool = "Write"
pattern = "*.env"
action = "deny"

[web]
enabled = true

[web.search]
provider = "brave"
api_key_env = "BRAVE_SEARCH_API_KEY"
timeout_secs = 15

[web.fetch]
timeout_secs = 20
redirect_limit = 5
max_response_bytes = 2097152
max_output_bytes = 65536

[compaction]
prune_protected_tokens = 8000
prune_min_savings = 4096
compact_threshold = 80000
compact_request_threshold = 90000
compact_retained_tokens = 8000
compact_auto_read_budget = 8000
compact_worker_max_input_tokens = 50000
compact_worker_max_turns = 20

[compaction.model]
scheme = "gemini"
model_id = "gemini-2.0-flash"
auth = { kind = "api_key", file = "/home/you/.config/insomnia/keys/gemini" }
```

---

## `[worker]` 設定

`[worker]` は Pod 内の `llm_worker::RequestConfig` とターン制御へ渡す設定を持つ。
Provider ごとの wire 名の違い（OpenAI の `max_completion_tokens` /
Responses の `max_output_tokens` / Gemini の `generation_config` など）は
scheme 側が吸収する。

| key | 型 | 既定 | 内容 |
|---|---|---|---|
| `instruction` | `String` | `$insomnia/default` | システムプロンプト本体として使う prompt asset 参照 |
| `max_tokens` | `u32` | 未指定 | 1 request の最大出力 token。scheme が provider の該当 wire field に投影。scheme ごとのセマンティクス差は `docs/reasoning.md` |
| `max_turns` | `NonZeroU32` | 未指定 | 1 run 内で Worker が進められる最大 turn 数 |
| `temperature` | `f32` | 未指定 | sampling temperature |
| `top_p` | `f32` | 未指定 | nucleus sampling |
| `top_k` | `u32` | 未指定 | top-k sampling。未対応 scheme では warning または provider 側挙動に任せる |
| `stop_sequences` | `Vec<String>` | `[]` | stop sequence。cascade では上層指定が配列ごと置換する |
| `reasoning` | `String` または `i32` | 未指定 | reasoning / thinking 制御。詳細は `docs/reasoning.md` |
| `tool_output.default_max_bytes` | `usize` | `65536` | tool result `content` の既定 byte cap |
| `tool_output.per_tool` | `Map<String, usize>` | `{}` | tool 名ごとの byte cap override |
| `file_upload.max_bytes` | `usize` | `262144` | submit 時の FileRef (`@<path>`) upload / attachment の byte cap |

生成設定は provider 別の値域検証を行わない。型が TOML と合わない場合は manifest
parse error になるが、provider が受け付けない値や組み合わせは API 応答で検出する。

## `[web]` 設定

`WebSearch` / `WebFetch` は通常の built-in function tool として登録されるが、manifest で明示的に有効化されるまでネットワークアクセスしない。無効または未設定の場合、tool call は「設定されていない」旨の明示的なエラーを返す。

```toml
[web]
enabled = true

[web.search]
provider = "brave"
api_key_env = "BRAVE_SEARCH_API_KEY" # API key は env 参照に置き、manifest に raw secret を書かない
timeout_secs = 15

[web.fetch]
timeout_secs = 20
redirect_limit = 5
max_response_bytes = 2097152
max_output_bytes = 65536
```

`WebSearch` の最初の provider は Brave Search API（`https://api.search.brave.com/res/v1/web/search`）で、入力は `query` と任意の `limit` / `offset`。Brave の制約に合わせて `query` は 400 文字 / 50 words まで、`limit` は 1-20、`offset` は 0-9 に制限される。`timeout_secs` を省略した場合は安全な既定値が使われ、provider response は固定上限内で読み込まれる。

`WebFetch` は http/https URL のみを fetch し、timeout・redirect・response/output byte limit を適用する。localhost / private / link-local などの host/IP は fetch 前と各 redirect で拒否される。テストや明示的に信頼した環境では `[web] allow_private_addresses = true` または `[web.fetch] allow_private_addresses = true` を指定できる。

## `[permissions]` 設定

`[permissions]` が無い場合、ツール permission 層は無効で従来通り実行する。`[permissions]` を書く場合は `default_action = "allow" | "deny" | "ask"` が必須で、`[[permissions.rule]]` は宣言順に最初に一致した rule が採用される。一致しなければ `default_action` を使う。

```toml
[permissions]
default_action = "allow"

[[permissions.rule]]
tool = "Bash"
pattern = "rm *"
action = "deny"
```

`tool` は実行時に登録されているツール名（`Bash`, `Read`, `Write`, `Edit`, `Glob`, `Grep`, `WebSearch`, `WebFetch` 等）に対して大小文字を無視して照合する。`pattern` は built-in tool では主に `command` / `file_path` / `path` / `pattern` / `query` / `url` 引数に対する `*` / `?` ワイルドカードとして評価される。

`allow` は通常実行、`deny` はその tool call を実行せず `is_error = true` の synthetic tool result を履歴へ追加してターンを継続する。`ask` は型として受け付けるが、承認 protocol は未実装のため現在は headless に待機せず fail-closed（synthetic error result）になる。

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

## `insomnia-pod` CLI

`insomnia-pod` の通常起動は profile discovery/default から runtime manifest を作る。user/project `manifest.toml` の ambient cascade は通常起動では使わない。

```
insomnia-pod [--profile <selector>] [--profile-pod-name <name>] [-s/--store <path>] [--session <uuid>]
```

| フラグ | 説明 |
|---|---|
| `--profile <selector>` | builtin/user/project profile registry から Nix profile を選択。省略時は registry default（通常は `builtin:default`） |
| `--profile-pod-name <name>` | profile 由来 manifest の `pod.name` を fresh spawn 用に上書き |
| `-s, --store <path>` | セッション永続化ディレクトリ（デフォルト: `<data_dir>/sessions/`、`manifest::paths` で解決） |
| `--session <uuid>` | 既存 session id から Pod を復元し、同じ jsonl に後続 turn を追記する |

単一ファイルだけで起動したい場合は `--manifest` を指定する。

```
insomnia-pod --manifest <path> [-s/--store <path>] [--session <uuid>]
```

`--manifest` は指定 TOML 1 枚だけを読み、builtin defaults を merge したうえで `PodManifestConfig -> PodManifest` の required validation を通す。user / project manifest layer は読まない。`--profile`、`--project` とは併用不可。

spawn 子 Pod 用の内部フラグとして `--adopt` と `--callback <path>` がある。これらは `SpawnPod` が scope allocation と親 callback socket を引き継がせるために使うもので、通常の手動起動では使わない。

Pod の作業ディレクトリは `insomnia-pod` 起動時の cwd が直接使われる。別ディレクトリで
動かしたい場合は `cd <path> && insomnia-pod ...` のように外側で `cd` してから起動する。

引数無しで起動すると、profile registry default（通常は bundled `builtin:default`）で起動する。

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
