# Pod Factory: 設定カスケードとプロンプト資産による Pod 自動生成

## レビュー状態

初回レビュー実施済み。[pod-factory.review.md](pod-factory.review.md) を参照。
要件全項目達成、アーキテクチャ・テスト被覆とも良好で**無条件で受け入れ可**。指摘は 6 件（すべて任意修正または nit レベル）。

## 背景

現状、Pod を起動するには `test_pod.local.toml` のような完全な `PodManifest` TOML を手書きする必要がある。1 人のユーザーが1 つのエージェントを試験運用するには十分だが、Insomnia が狙う「複数のエージェントが独立プロセスとして spawn されて自律的に動く」世界観では、**Pod のライフサイクル全体が自動化可能でなければならない**。そのためには、Pod の**作成自体**も自動化可能である必要がある。

手書きマニフェストには以下の問題がある:

- 1 Pod = 1 ファイルで、Pod を動的に増やす用途にスケールしない
- 設定項目が多く（`worker.*` / `provider.*` / `scope.*` / `compaction.*` / `tool_output.*` 等）、毎回コピペしてわずかな差分だけ書き換える苦行になる
- system_prompt を TOML 文字列に埋め込む形はプロンプト資産の再利用性が低い
- Pod の起動条件の**共通部分**（プロバイダ・モデル・デフォルトツール設定など）は本来一度書けば良いのに、毎回書かされる

## ゴール

Pod 作成を「**最終的に `PodManifest` を1 つ構築する問題**」として定式化し、その `PodManifest` を**カスケード + 差分上書き**で組み立てる基盤を提供する。手書きが必要な TOML は「ユーザー / プロジェクト単位の**デフォルト上書き**」だけに縮退させ、個別の Pod 起動ごとに人間が TOML を触らない状態を目指す。

プロンプトは手書きマニフェストに文字列を埋め込む方式をやめ、**テンプレート資産ライブラリ**として参照可能にする。

## 方針

### 同じ型で、層で上書きする

- **解決後の型は現行の `manifest::PodManifest` のまま**（必須フィールド持ち）。
- カスケードの各層は **`PodManifestConfig`**（`PodManifest` と同じ構造だが全フィールドを `Option` / 部分形で保持する新規型）で表現する。部分形同士を順番にマージし、最後に `TryFrom<PodManifestConfig> for PodManifest` で必須チェックを行いつつ確定させる。
- 各層は**部分形**を持てる（全フィールドを埋める必要はない）。存在するフィールドだけが下層を上書きする。
- 人間が書くときは `PodManifest` と同じ TOML スキーマで書く（サブセット可）。ファイル名は**すべて `manifest.toml`**。「設定」という曖昧な名前を避け、「Pod manifest」という語彙に揃える。

### カスケードの層

優先順位が低い方から高い方へ:

1. **ビルトインのデフォルト**: コードに焼き込んだ基本値（現在 `PodManifest` 各フィールドの `#[serde(default)]` や `Default` 実装に散っているものを集約）
2. **ユーザー manifest**: `$XDG_CONFIG_HOME/insomnia/manifest.toml`（`XDG_CONFIG_HOME` 未設定時は `~/.config/insomnia/manifest.toml`）。ユーザー個人のプロバイダ指定・デフォルトモデル・常用ツール設定等を書く
3. **プロジェクト manifest**: プロジェクトルート下の `.insomnia/manifest.toml`。プロジェクト固有の scope・compaction 設定・system_prompt のベース等を書く
4. **プログラマティック overlay**: Pod 生成を呼ぶコード（GUI / CLI / 別 Pod からの spawn 等）が渡す部分形。ここで `pod.name` や `pod.pwd` のような**その Pod に固有の値**を与える

### プロジェクトルートの判定

起動ディレクトリから上方向に `.insomnia/` ディレクトリを探索し、**最も近い**ものをプロジェクトルートとする。見つからなければ「プロジェクト無し」として扱い、プロジェクト層をスキップする。

### マージのセマンティクス

- **スカラー** (`String`, `u32`, `bool` 等): 上層が存在すれば丸ごと置換
- **Option 型**: 上層が `Some` なら置換、`None` なら据え置き
- **マップ** (例: `tool_output.per_tool`): キー単位でマージ、同一キーは上層優先
- **`scope.allow` / `scope.deny`**: **union（各層から全部足す）**。すべての層が追加のみ可能。最終的な effective scope は既存の `Scope::from_config` ロジックに委ね、`allow_union - deny_union` として解決される。**上位層は deny を追加することで下位層の allow を必ず削れる**ので、「上に行くほど強制力を持てる」構造が自然に出る
- その他リスト: 当面は `scope.*` 以外にリスト型は存在しないが、新規にリスト型フィールドを追加するときは原則 union（scope と同じ）で扱う

### エラー戦略

- **未知フィールド**: TOML に書かれた未定義のキーは `tracing::warn!` のみ出して無視する。`#[serde(deny_unknown_fields)]` は**使わない**（将来バージョンアップで読めない旧設定が出るとユーザー体験が悪いため）
- **型ミスマッチ**: `max_tokens = "100"` のような型エラーは hard error として resolve 失敗させる。ファイルパスと位置情報をエラーメッセージに含める
- **パスフィールド**: `pod.pwd` / `provider.api_key_file` / `scope.*.target` 等のパスは**絶対パスのみ受け付ける**。相対パスが書かれていたら resolve 時に hard error とする。相対パス解決を層ごとの manifest_dir に依存させるとカスケードの意味論が濁るため、各ファイルを書く人間の責任で絶対パスを指定させる

### プロンプト資産ライブラリ

- プロンプトは TOML 文字列ではなく**ファイルとして管理**する。
- 検索パスは以下の3層（上層優先で解決）:
  1. **ビルトイン**: レポジトリ直下の `resources/prompts/` 以下を `include_dir!` マクロでバイナリに同梱
  2. **ユーザー**: `$XDG_CONFIG_HOME/insomnia/prompts/`
  3. **プロジェクト**: `<project>/.insomnia/prompts/`
- **ファイル名の stem がそのままプロンプト名**。サブディレクトリも許容し、`resources/prompts/common/tool-usage.md` は `{% include "common/tool-usage" %}` で参照できる
- ファイル形式は **`.md`**（frontmatter は無し。前方互換として `---` 区切りの TOML フロントマターを将来持てる余地だけ認識しておく）
- 既存の `SystemPromptTemplate`（minijinja ベース）の `Environment` にカスタムローダを仕込み、テンプレート内から他のプロンプトを `{% include "name" %}` / `{% import "name" as p %}` で参照できるようにする
- プロンプト資産自体もテンプレートとして評価され、現行の `SystemPromptContext`（`now` / `cwd` / `scope` / `tools` / `files` 等）と同じ変数が見える
- include 時の**変数伝搬は minijinja のデフォルト挙動**（親スコープの変数が自動で見える）に任せる

### 設定値のテンプレート参照は扱わない

`worker.max_tokens = "{{ env.INSOMNIA_MAX_TOKENS }}"` のような**設定値の中でテンプレートを展開する機能は本チケットの範囲外**とする。テンプレートエンジンはプロンプト本文の組み立てだけに限定する。設定値の動的化が必要になった時点で別チケットで検討する。

### プログラマティック Pod 作成 API

factory は `PodManifest` を組み上げる builder として設計する。イメージ:

```rust
let manifest: PodManifest = PodFactory::new()
    .with_user_manifest_auto()?      // XDG から自動読み込み、不在 OK
    .with_project_manifest_auto()?   // cwd から上方向に .insomnia/ を探索、不在 OK
    .with_overlay_toml(overlay)?     // programmatic な最上層 overlay（TOML 文字列）
    .resolve()?;                     // -> PodManifest
Pod::from_manifest(manifest, store).await?;
```

`Pod::from_manifest` は現在 `manifest_dir` を取るが、本チケットで以下の二段に書き換える:

- **`Pod::from_manifest(manifest: PodManifest, store)`** — 一次 API。factory の出力がそのまま入る
- **`Pod::from_manifest_toml(toml: &str, store)`** — 便利関数。単層 manifest を TOML 文字列で直接投げる用途（テスト・デバッグ・動作確認）

`manifest_dir` 引数は廃止する。パスは絶対化済み前提のため、Pod 側で相対パス解決ロジックを持たない。ファイル読み込みは呼び出し側（factory か caller）の責務。

### CLI

`pod` バイナリの CLI surface を factory に合わせて更新する。新 umbrella コマンド（`insomnia spawn` 等）は**作らない**。ユーザーが直接 CLI を叩くのは debug/automation の文脈で、GUI/TUI は Pod を subprocess として spawn する運用のため、`pod` バイナリ1本の fla だけで十分。

```
pod [--user-manifest <path>] [--project <path>] [--overlay <toml>] [--pwd <path>]
```

- `--user-manifest`: 省略時は XDG から自動解決
- `--project`: 省略時は cwd から上方向に `.insomnia/` を探索
- `--overlay`: 最上層の overlay を inline TOML 文字列で渡す（例: `--overlay 'pod.name = "dbg"'`）
- `--pwd`: 便利ショートカット（overlay に `pod.pwd = ...` を書く代わり）
- 旧 `--manifest <path>` フラグは廃止。単層 manifest を直接読み込みたい場合は `--user-manifest <path>` で代替する

## 要件

### カスケード基盤

- ユーザー manifest・プロジェクト manifest・プログラマティック overlay を順に重ねた結果が `PodManifest` として取れる。
- 各層が部分形を許容する（`pod.pwd` だけ書いてあっても良い等）。
- マージセマンティクスがフィールドごとに定義され、単体テストで担保される（スカラー / Option / マップ / scope union）。
- 未知フィールドは warn のみ、型ミスマッチは hard error。
- 層が全て空で resolve 失敗する場合は、必要なフィールド（`pod.pwd` / `provider` / `scope.allow` 等）を明示するエラーを返す。

### プロンプト資産ライブラリ

- 3層の検索パスでプロンプトファイルを解決できる。
- 同名プロンプトは上層優先で解決される。
- `SystemPromptTemplate` の minijinja `Environment` にカスタムローダを仕込み、`{% include "name" %}` / `{% import "name" as x %}` で資産を参照できる。
- 親テンプレートの変数が include 先にも見える（minijinja デフォルト挙動）。
- ビルトインプロンプトは `resources/prompts/` から `include_dir!` マクロで同梱され、追加・編集するときは該当ディレクトリにファイルを置く・編集するだけで済む。
- プロンプト資産自体もテンプレートとして評価され、`SystemPromptContext` と同じ変数が見える。

### プログラマティック Pod 作成

- `Pod::from_manifest(manifest: PodManifest, store)` と `Pod::from_manifest_toml(toml: &str, store)` の二段構成。path 受け API は廃止。
- `PodFactory` builder が上記に繋がる。
- TUI / GUI / daemon 等の上位クライアントが、TOML ファイルパスではなく overlay + 設定ディレクトリパスを渡すだけで Pod を起動できる。

### CLI

- `pod` バイナリが `--user-manifest` / `--project` / `--overlay` / `--pwd` を受け付ける。
- 引数無しで cwd + XDG を自動解決して起動できる最小構成が動く。

### ドキュメント

- カスケード層の優先順位・マージ規則を `docs/` にまとめる。
- ユーザー manifest / プロジェクト manifest の最小例と全オプション例を残す。
- `resources/prompts/` のディレクトリ構造とビルトインプロンプト一覧を `docs/` に記載。

## リソース構成

```
insomnia/
├── crates/
│   ├── pod/
│   │   └── src/
│   │       ├── factory.rs        # 新規。PodManifestConfig カスケード → PodManifest
│   │       ├── prompt_loader.rs  # 新規。minijinja loader + 3層検索
│   │       └── ...
│   └── ...
├── resources/
│   └── prompts/
│       ├── coder.md
│       ├── reviewer.md
│       ├── planner.md
│       └── common/
│           └── tool-usage.md
```

- factory とプロンプト loader は新規 crate を作らず `pod` crate 内に配置する。既に `pod` が `manifest` に依存しているため追加の依存関係整理が不要
- `include_dir!("$CARGO_MANIFEST_DIR/../../resources/prompts")` で全ビルトインプロンプトをバイナリに取り込む
- ファイルの追加・編集は通常の file 操作だけで済む。既存ファイルの編集は cargo の変更追跡で自動再ビルドされる。新規追加・削除が反映されない場合は `cargo clean -p pod` を案内する（必要が増えたら `build.rs` で `rerun-if-changed` を足す）
- ビルトインプロンプトの初期ラインナップ（`coder` / `reviewer` / `planner` / `common/tool-usage` 等）は実装時に選定し、ドキュメントに列挙する

## 完了条件

- `PodFactory` がカスケード層のマージで `PodManifest` を構築でき、マージセマンティクスの単体テストが通る（スカラー・Option・マップ・scope union・未知フィールド warn・型ミスマッチ hard error）。
- ユーザー manifest・プロジェクト manifest・プログラマティック overlay のすべての層を使う end-to-end テストで、Pod が path を一切渡さずに起動できる。
- プロンプト資産ライブラリを経由して system_prompt が組み立てられ、`{% include "ビルトイン名" %}` で `resources/prompts/` 以下の資産を参照できることをテストで確認できる。
- `Pod::from_manifest(manifest)` と `Pod::from_manifest_toml(toml)` の二段 API が動き、path 受け API は削除されている。
- `pod` バイナリが `--user-manifest` / `--project` / `--overlay` / `--pwd` を受け、引数無しでも cwd + XDG 自動解決で起動できる。
- カスケード層の優先順位・マージ規則 / ユーザー manifest 例 / プロジェクト manifest 例 / ビルトインプロンプト一覧のドキュメントが `docs/` に存在する。

## 他チケットとの関係

- `tickets/native-gui-mvp.md`: 現状「manifest ファイルを選ぶ UI」を含むが、本チケット完了後はその UI が「overlay 指定 + Pod 起動」に置き換わる想定。native-gui-mvp 実装時に本チケットの API を使うか、先に path 渡しで済ませて後から差し替えるかは別途判断
- `tickets/tui-pod-spawn-ui.md`: 同上。Pod spawn UI は本チケットが提供する API の上に構築される
- `tickets/protocol-design.md`: Pod ↔ Client protocol 自体は変わらない。spawn 要求を protocol に載せるかどうかは protocol-design 側で検討
- `docs/system-prompt-template.md` / `crates/pod/src/system_prompt.rs`: プロンプト資産ライブラリはこの minijinja 基盤の拡張として実装される

## 範囲外

- **preset の概念**: 名前付き overlay セット（`insomnia spawn coder` 等）は本チケットでは導入しない。将来別チケットで検討
- **設定値の中のテンプレート展開**（`max_tokens = "{{ env.X }}"` のような動的値）。プロンプト本文のテンプレート展開のみを扱う
- **GUI 内での manifest 編集 UI**。編集は人間がエディタで TOML を書くだけ（あくまで「Pod 生成時に手書きしない」ことを目指す）
- **チーム共有・同期**。ユーザー manifest とプロジェクト manifest は各自・各リポジトリ単位で管理される
- **秘密情報管理**（API キー等）。既存の `api_key_file` 方式を維持する
- **設定値の型バリデーション強化**（JSON Schema など）。現行の serde ベースで十分な範囲に留める
- **プロンプトフロントマター**（description / required_vars 等のメタデータ）。将来一覧 UI が必要になった時点で検討。フォーマット上の前方互換性だけは意識する
- **umbrella CLI コマンド**（`insomnia spawn` 等）。`pod` バイナリ単体で完結させる
