# Pod Factory: 設定カスケードとプロンプト資産による Pod 自動生成

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

- **解決後の型は現行の `manifest::PodManifest` のまま**。Pod 側の契約（`Pod::from_manifest`）は変更しない。
- カスケードは `PodManifest` より上の「部分的な `PodManifest` を層ごとに保持し、順番にマージして最終形を作る」層として設計する。
- 各層は**部分形**を持てる（全フィールドを埋める必要はない）。存在するフィールドだけが下層を上書きする。

### カスケードの層

優先順位が低い方から高い方へ:

1. **ビルトインのデフォルト**: コードに焼き込んだ基本値（現在 `PodManifest` 各フィールドの `#[serde(default)]` や `Default` 実装に散っているものを集約）
2. **ユーザー設定**: `~/.config/insomnia/config.toml` など。ユーザー個人のプロバイダ指定・デフォルトモデル・常用ツール設定等を書く
3. **プロジェクト設定**: プロジェクト直下の `.insomnia/config.toml` など。プロジェクト固有の scope・compaction 設定・system_prompt のベース等を書く
4. **プログラマティック上書き**: Pod 生成を呼ぶコード（GUI / CLI / 別 Pod からの spawn 等）が渡す `PodManifestOverlay` 的な部分形。ここで `pod.name` や `pod.pwd` のような**その Pod に固有の値**を与える

各層とも人間が書くときは `PodManifest` と同じ TOML スキーマで書く（サブセット可）。

### マージのセマンティクス

- **スカラー** (`String`, `u32`, `bool` 等): 上層が存在すれば丸ごと置換
- **Option 型**: 上層が `Some` なら置換、`None` なら据え置き
- **マップ** (例: `tool_output.per_tool`): キー単位でマージ、同一キーは上層優先
- **リスト** (例: `scope.allow` / `scope.deny`): **原則置換**（append にすると下層の意図しないルールが漏れる危険）。ただし append したいケースはあるので、設計時に decoration の形式（例: `scope.allow_extra` など）を別途検討
- 未知フィールドは manifest エラーにせずログ警告

### プロンプト資産ライブラリ

- プロンプトは TOML 文字列ではなく**ファイルとして管理**する。
- 検索パスはカスケードと対応した3層:
  1. **ビルトイン**: バイナリに同梱されたデフォルトプロンプト（`coder` / `reviewer` / `planner` 等、設計時に選定）
  2. **ユーザー**: `~/.config/insomnia/prompts/*.md` 等
  3. **プロジェクト**: `.insomnia/prompts/*.md` 等
- 同名があれば**上層が優先**。
- 既存の `SystemPromptTemplate`（minijinja ベース）のローダを拡張し、テンプレート内から他のプロンプトを `{% include "coder" %}` / `{% import "planner" as p %}` のように参照できるようにする。
- 層の異なる同名プロンプトを合成するための include 先解決は上記優先順位に従う。

### 設定値のテンプレート参照は扱わない

`worker.max_tokens = "{{ env.INSOMNIA_MAX_TOKENS }}"` のような**設定値の中でテンプレートを展開する機能は本チケットの範囲外**とする。テンプレートエンジンはプロンプト本文の組み立てだけに限定する。設定値の動的化が必要になった時点で別チケットで検討する。

### プログラマティック Pod 作成 API

- `Pod::from_manifest(path)` の隣に、カスケード解決を経由する生成経路を追加する。イメージ:

  ```rust
  // 層を明示的に指定して最終形を得る
  let manifest = PodFactory::new()
      .with_user_config(user_config_path)?     // absent OK
      .with_project_config(project_root)?      // absent OK
      .with_overlay(overlay_toml_or_struct)    // programmatic
      .resolve()?;                             // -> PodManifest
  Pod::from_manifest(manifest, store).await?;
  ```

- 細かい形状は設計時に詰める（builder 型 vs 関数型、`overlay` を TOML 文字列か型付きか等）。
- CLI からは `insomnia spawn --overlay '...'` 相当で同じ経路を叩く想定。

## 要件

### カスケード基盤

- ユーザー設定・プロジェクト設定・プログラマティック overlay を順に重ねた結果が `PodManifest` として取れる。
- 各層が部分形を許容する（`pod.pwd` だけ書いてあっても良い等）。
- マージセマンティクスが**フィールドごとに定義**され、テストされる（スカラー / Option / マップ / リスト）。
- 層が全て空でも**ビルトインデフォルト単体で有効な manifest にならない**（少なくとも `pod.pwd` と `provider` と `scope.allow` は上位層で与える必要がある）。その旨を resolve 時のエラーで明示する。

### プロンプト資産ライブラリ

- 3層の検索パスでプロンプトファイルを解決できる。
- 同名プロンプトは上層優先で解決される。
- `SystemPromptTemplate` の minijinja `Environment` にカスタムローダを仕込み、`{% include "name" %}` / `{% import "name" as x %}` で資産を参照できる。
- プロンプト資産自体もテンプレートとして評価され、現行の `SystemPromptContext`（`now` / `cwd` / `scope` / `tools` / `files` 等）と同じ変数が見える。

### プログラマティック Pod 作成

- 既存の `Pod::from_manifest` を壊さず、追加経路として `PodFactory` 系の API を提供する。
- TUI / GUI / daemon 等の上位クライアントが、TOML ファイルパスではなく**オーバーレイ + 設定ディレクトリパス**を渡すだけで Pod を起動できる。

### ドキュメント

- カスケード層の優先順位・マージ規則を `docs/` にまとめる。
- ユーザー設定 / プロジェクト設定ファイルの**最小例**と**全オプション例**を残す。

## 設計で決めること

- **ユーザー設定のパス**: `~/.config/insomnia/config.toml` か、XDG 非準拠のパスも許容するか。環境変数で上書きできるか。
- **プロジェクト設定の場所**: プロジェクトルートの `.insomnia/config.toml` か、別の命名か。サブディレクトリから起動したときの discovery（上位ディレクトリ探索）の挙動。
- **プロジェクトルートの判定**: 明示指定 vs `.git` や `.insomnia/` で自動検出
- **preset の概念を入れるか**: 名前付きの overlay セット（例: `insomnia spawn coder`）を導入するか。導入する場合、preset はユーザー設定内に `[preset.coder]` として持つか、個別ファイル `~/.config/insomnia/presets/coder.toml` として持つか
- **リストフィールドのマージ方針**: 置換 only にするか、append 用の別フィールド (`scope.allow_extra` 等) を用意するか
- **ビルトインプロンプトの初期ラインナップ**: どの役割をデフォルトで同梱するか、どこに置くか（`crates/pod/assets/prompts/*.md` を `include_str!` で埋め込む等）
- **プロンプト資産のファイル形式**: `.md` か `.txt` か、拡張子省略可能にするか、フロントマター（YAML/TOML）で引数デフォルトを持たせるか
- **プロンプト include 時の context 伝搬**: 親テンプレートの変数を include 先でも見えるようにするか、明示的に `with` で渡させるか
- **エラー戦略**: 上層で書かれた未知フィールドや型ミスマッチをどこまで寛容に扱うか
- **既存の `Pod::from_manifest(path)` とのインターフェース整理**: 廃止するか、内部的に PodFactory に委譲するか
- **CLI コマンド名**: `insomnia spawn` / `insomnia pod new` / その他

## 完了条件

- `PodManifest` の最終形を層のマージで構築する `PodFactory`（または同等の仕組み）が実装され、マージセマンティクスの単体テストが通る。
- ユーザー設定・プロジェクト設定・プログラマティック overlay のすべての層を使う end-to-end テストで、Pod が TOML ファイルパスを一切渡さずに起動できる。
- プロンプト資産ライブラリを経由して system_prompt が組み立てられ、`{% include "ビルトイン名" %}` で同梱プロンプトを参照できることをテストで確認できる。
- ユーザー設定ファイル / プロジェクト設定ファイルのドキュメントが `docs/` に存在する。
- 既存の `Pod::from_manifest(path)` 経路が動き続ける（回帰させない）。

## 他チケットとの関係

- `tickets/native-gui-mvp.md`: 現状「manifest ファイルを選ぶ UI」を含むが、本チケット完了後はその UI が「preset 選択 + overlay 入力」に置き換わる想定。native-gui-mvp 実装時に本チケットの API を使うか、先に文字列パス渡しで済ませて後から差し替えるかは別途判断
- `tickets/tui-pod-spawn-ui.md`: 同上。Pod spawn UI は本チケットが提供する API の上に構築される
- `tickets/protocol-design.md`: Pod ↔ Client protocol 自体は変わらない。spawn 要求を protocol に載せるかどうかは protocol-design 側で検討
- `docs/system-prompt-template.md` / `crates/pod/src/system_prompt.rs`: プロンプト資産ライブラリはこの minijinja 基盤の拡張として実装される

## 範囲外

- **設定値の中のテンプレート展開**（`max_tokens = "{{ env.X }}"` のような動的値）。プロンプト本文のテンプレート展開のみを扱う
- **GUI 内での設定ファイル編集 UI**。編集は人間がエディタで TOML を書くだけ（あくまで「Pod 生成時に手書きしない」ことを目指す）
- **チーム共有・同期**。ユーザー設定とプロジェクト設定は各自・各リポジトリ単位で管理される
- **秘密情報管理**（API キー等）。既存の `api_key_file` 方式を維持する
- **設定値の型バリデーション強化**（JSON Schema など）。現行の serde ベースで十分な範囲に留める
