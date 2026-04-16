# instruction のファイル参照化と system prompt 末尾の固定セクション

## 背景

pod-factory で `worker.system_prompt` を minijinja 文字列として扱う形にしたが、その後の運用方針の整理で以下のズレが見えてきた:

- **manifest を手で触らない前提**なので、`system_prompt` に minijinja 文字列をインラインで書く設計そのものが過剰。人間が書くのはプロンプトの**中身ファイル**だけで十分
- **AGENTS.md を `files.agents_md` として user template に露出**しているが、ユーザー領域のテンプレートに AGENTS.md 埋め込みを任せると、guard 忘れで静かに消えたり、preset を差し替えるたびに AGENTS.md の扱いが破綻する
- **scope はセキュリティ境界**なので、user の template が覚えていてくれる前提にすべきでない
- **prompt loader が by-name fallthrough**（project → user → builtin を順に探索）になっており、「同名上書き」と「偶然同名」が区別できない曖昧さがある
- 現在バイナリに同梱されている `coder` / `reviewer` / `planner` / `common/tool-usage` は AI 任せで書いた placeholder で、設計者の意図が乗っていない
- 既存の `Scope::summary()` は許可されたパスを列挙するだけで、**`recursive = false` の情報が失われて**おり、非再帰ルールの意味が LLM に伝わらない

## ゴール

1. `worker.system_prompt` を**ファイル参照フィールド** (`worker.instruction`) に置き換える
2. `$insomnia` / `$user` / `$workspace` の **import-map 形式の prefix** でプロンプト資産を addressing
3. scope と AGENTS.md を **コード側で system prompt 末尾に付加する固定セクション**として注入し、user template が触れない領域にする
4. 組み込みプロンプトを `$insomnia/default` の 1 本に整理
5. `Scope::summary()` のフォーマットを改善し、`recursive = false` を明示する

## 方針

### instruction フィールド

- manifest schema: `worker.system_prompt: Option<String>` を **`worker.instruction: Option<String>`** に置き換える（minijinja 文字列を持つフィールドを消し、ファイル参照だけを受ける）
- 値は import-map 記法のファイル参照: `$insomnia/default` / `$user/my-style` / `$workspace/custom`
- `.md` 拡張子は省略する
- **デフォルト値**は `$insomnia/default`。manifest で `instruction` を書かなければこれが使われる（`defaults.rs` に `DEFAULT_INSTRUCTION: &str = "$insomnia/default"` を追加）
- サブディレクトリ許容: `$insomnia/common/header` は `resources/prompts/common/header.md` を指す

### Prefix 解決

| prefix | 解決先 |
|---|---|
| `$insomnia` | バイナリ同梱の `resources/prompts/`（`include_dir!`） |
| `$user` | `$XDG_CONFIG_HOME/insomnia/prompts/`（factory が設定した user prompts dir） |
| `$workspace` | `<project>/.insomnia/prompts/`（factory が設定した project prompts dir） |

- 指定した prefix の dir に該当ファイルが無ければ **hard error**（fallthrough しない）
- 現在の「by-name で層を fallthrough して探す」ロジックは撤廃

### Unqualified include の相対解決

`{% include "name" %}` のように prefix 無しで書かれた場合は、**include を書いたファイル自身の prefix + ディレクトリからの相対**で解決する:

- `$insomnia/default.md` 内の `{% include "sub" %}` → `$insomnia/sub`
- `$insomnia/common/header.md` 内の `{% include "nested/foo" %}` → `$insomnia/common/nested/foo`
- `$user/custom.md` 内の `{% include "$insomnia/default" %}` → 明示的 prefix が優先

これにより「同じディレクトリ内でかたまって動くプロンプト集」を自然に書ける。

### system prompt 末尾の固定セクション

`SystemPromptTemplate` のレンダリング後に、Rust 側で以下の構造を**必ず**付加する（user template からは触れない）:

```
<instruction file のレンダ結果>

---
## Working boundaries

{scope.summary()}
{% if agents_md %}
---
## Project instructions (AGENTS.md)

{agents_md 本文}
{% endif %}
```

- 付加部分は Rust の `const` またはハードコードされたフォーマット文字列として実装し、`resources/` には置かない（user が触れる場所に置くと、触らないでほしいものが触れる場所に置かれる矛盾になる）
- scope セクションは **必ず** 出力される
- AGENTS.md セクションは不在時に省略（区切り `---` ごと省略）

### `SystemPromptContext.files` の削除

- `files` フィールドごと撤廃（元々 AGENTS.md 専用の予約席）
- user template から AGENTS.md を参照する手段は無くなる（末尾セクションが面倒を見る）

### `Scope::summary()` フォーマットの改善

`crates/manifest/src/scope.rs` の `Scope::summary()` を以下の方針で書き直す:

- **非再帰ルールを `[non-recursive]` でマークする**。例:
  ```
  Readable:
    - <local-path> [non-recursive]
  Writable:
    - <repo>
  ```
- マーカーの位置はパスの末尾（パースしやすさより人間可読性を優先）
- recursive = true の場合は何も足さない（デフォルトなので無印）
- deny ルールは現行どおり summary には出さない（`from_config` の時点で effective permission に焼き込まれるため）
- `Readable` / `Writable` のセクション分けは現行どおり

この変更により、最終セクションが出力する scope 情報が LLM にとって正確になる。

### 組み込みプロンプトの整理

- **削除**: `resources/prompts/coder.md` / `reviewer.md` / `planner.md` / `common/tool-usage.md`
- **新規**: `resources/prompts/default.md` 1 本のみ（内容は本チケットの実装時に author が記述）
- 将来、役割ごとの preset を復活させたくなったら別チケットで追加する（preset 概念そのものが pod-factory の範囲外だった経緯もあり、安易に戻さない）

## 要件

### Schema

- `manifest::WorkerManifest` と `manifest::WorkerManifestConfig` から `system_prompt` を削除、`instruction: Option<String>` を追加（部分形は Option、resolve 時に `defaults::DEFAULT_INSTRUCTION` で埋める）
- `manifest::defaults` に `DEFAULT_INSTRUCTION: &str = "$insomnia/default"` を追加

### Loader

- `PromptLoader` を prefix addressing 版に書き換える
- API: `PromptLoader::resolve(ref: &str, current: Option<&PromptRef>) -> Result<String, Error>`
  - `ref` が `$prefix/path` 形式なら該当 dir を引く
  - 素の `name` なら `current` の prefix + dir を前置して再帰 resolve
  - `current` が無い状態で素の `name` が来たら **error**
- minijinja の `Environment::set_loader` 内で `current` を追跡する

### 末尾セクションの組み立て

- `SystemPromptTemplate::render` 相当の経路で、**user template 出力 + 固定末尾セクション**を連結する
- scope / AGENTS.md を formatter に渡す
- `Pod::ensure_system_prompt_materialized` の既存フローを素直に置き換える

### `Scope::summary()`

- `recursive = false` のルールに `[non-recursive]` マーカーを付ける
- 既存の `summary` 形式は維持（`Readable:` / `Writable:` のセクション + インデントのフォーマット）
- `crates/manifest/src/scope.rs` の既存テスト `summary_lists_readable_and_writable` / `summary_excludes_deny_rules` を更新または補完

### テスト

- 既存: `include_resolves_builtin_prompt` / `agents_md_is_injected_when_present` / `files_reserved_namespace_is_empty` / `files_map_*` 系を書き換えまたは削除
- 新規:
  - `instruction_default_resolves_to_insomnia_default`
  - `instruction_prefix_addressing_{insomnia,user,workspace}`
  - `include_unqualified_resolves_relative_to_current_prefix`
  - `include_explicit_prefix_overrides_relative`
  - `prefix_with_missing_file_is_hard_error`
  - `trailing_section_always_contains_scope_summary`
  - `trailing_section_contains_agents_md_when_present`
  - `trailing_section_omits_agents_md_when_absent`
  - `scope_summary_marks_non_recursive_rules`

### ドキュメント

- `docs/pod-factory.md` を更新:
  - `files.agents_md` の記述を削除
  - loader の by-name fallthrough 説明を prefix addressing に置き換え
  - `instruction` フィールドと `$insomnia/default` デフォルトの記述を追加
  - system prompt 末尾の固定セクション構造を図示
- 必要なら `docs/system-prompt-template.md` も追随

## 他チケットとの関係

- **pod-factory (完了済み)**: 本チケットは pod-factory で実装した `PromptLoader` の**一部書き換え**になる。特に「3 層 by-name fallthrough」は撤廃される。pod-factory 自体をロールバックしない
- **tickets/native-gui-mvp.md**: 直接の交差なし。GUI が instruction を差し替えたいときは同じ prefix 形式で渡せば良い
- **tickets/protocol-design.md**: protocol 非依存
- **tickets/agents-md-ingestion.md (完了済み)**: AGENTS.md 読み取り経路は維持。表面の user template 経路だけが消える

## 完了条件

- manifest `[worker]` から `system_prompt` が消え、`instruction` でファイル参照を渡す形になっている
- `instruction` を省略すれば `$insomnia/default` が使われる
- `$insomnia` / `$user` / `$workspace` の 3 種の prefix でプロンプト資産を参照できる
- prefix 無しの `{% include %}` が include 元ファイルの prefix に相対解決される
- 未知の prefix や不在ファイルは hard error になる
- Pod の system prompt は常に「instruction の render 結果 + scope 要約 + (AGENTS.md があれば)」の構造で組み立てられる
- `SystemPromptContext` から `files` が削除され、user template から AGENTS.md を参照する手段は残っていない
- `Scope::summary()` が `recursive = false` ルールに `[non-recursive]` マーカーを付ける
- `resources/prompts/` は `default.md` 1 本のみ
- 上記挙動がすべて単体テストで担保されている
- `docs/pod-factory.md` が新方針に追随している

## 範囲外

- **preset 概念の復活**: 削除した `coder` / `reviewer` / `planner` を preset として体系化する議論は別チケット
- **instruction をファイル参照以外にする拡張**: インライン minijinja、TOML 配列、等は入れない
- **CLI 変更**: `pod` バイナリの flag は `--overlay` で instruction を上書きできる（`--overlay 'worker.instruction = "$user/foo"'`）形でそのまま使える。新規 flag は追加しない
- **テンプレートエンジンの差し替え**: minijinja を維持
- **`default.md` 本文の著者判断**: ticket の範囲は枠組み。本文は実装者が書く
- **scope summary に deny ルールを出す改善**: 現行どおり deny は effective permission に焼き込むだけで、summary には出さない
