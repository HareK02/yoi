# システムプロンプトのテンプレート化

**Status:** Reviewed — 指摘事項は `system-prompt-template.review.md` 参照。

## 背景

現状、`WorkerManifest.system_prompt` は単なる `Option<String>` で、マニフェスト記述時点の固定テキストしか持てない。実行時に決まる情報（日付、cwd、scope、利用可能なツール、外部ファイルの内容など）をシステムプロンプトに埋め込む手段が無く、Pod ごとに文脈を調整したいケースに対応できない。

AGENTS.md の取り込みをはじめ、今後システムプロンプトへ差し込みたい情報は増えていく見込みで、その受け皿としてテンプレート機構を先に固める。

プロジェクト内には既に2種類の遅延初期化パターンがある:
- **Tool (llm-worker 層)**: `register_tool` は factory を `pending` に積むだけで、`Worker::lock()` の `flush_pending` が first turn 直前に一括 materialize する。
- **Hook (Pod 層)**: `hook_builder.add_*` は `HookRegistryBuilder` に積むだけで、`Pod::run` 冒頭の `ensure_interceptor_installed` が `builder.build()` → `worker.set_interceptor` を呼ぶ。1回性は bool フラグで担保。

システムプロンプトのテンプレートは **hook と完全に対称な Pod 層の ensure_\* パターン** に乗せる。Worker は低レベル基盤に留めるため、テンプレートの存在を知らない。

## 要件

### 評価モデル

- マニフェストの値はテンプレート定義として `Pod` に保持し、文字列への materialize は **first turn 開始時に1回だけ** 行う。
- `Pod::run` 冒頭の `ensure_interceptor_installed` の隣に `ensure_system_prompt_materialized` を追加する。`Option::take()` で構造的に1回性を担保し、materialize 済みなら早期 return。
- 一度 render した文字列は `worker.set_system_prompt` 経由で Worker 側の既存フィールドに乗る。以降のターン、および **compact 後も再評価しない**（compact は Worker の system_prompt フィールドを触らないので、Pod 側の template が `None` になっている限り再 render は構造上不可能）。

### テンプレート構文と変数

- **テンプレートエンジン**: minijinja を採用する。理由:
  - `{{ var }}` / `{% if %}` / `{% for %}` / filter が使え、AGENTS.md の有無で条件分岐したい将来要件にそのまま乗る
  - `UndefinedBehavior::Strict` で未定義変数参照を明示エラーにできる（fail-fast に一致）
  - エスケープは `{{ '{{' }}` で Jinja2 標準
  - Pure Rust、依存少、メンテ活発
- 組み込み変数の初期セット:
  - `date` / `time` / `datetime`（ISO8601 / RFC3339）
  - `cwd`（Pod の絶対パス）
  - `scope` — `{ readable: [...], writable: [...], summary: "..." }`
  - `tools` — ツール名の sort 済みリスト
  - `files` — AGENTS.md 等の外部ファイル用に予約キー空間（別チケットで値を供給する前提で、本チケットでは常に空 Map）
- 未定義変数参照は render エラーとして失敗。
- `{{` のリテラル出力は `{{ '{{' }}` で可能。

### マニフェスト上の記法

- 既存フィールド `WorkerManifest.system_prompt: Option<String>` をそのままテンプレート文字列として解釈する。新フィールドは作らない。
- マニフェストのパース段階ではテンプレート構文検査のみ行う。値の解決は行わない。

### 責務の分離

- テンプレート機構は **Pod 層** に閉じる。llm-worker は低レベル基盤の原則を維持し、テンプレートの存在を知らない。
- Worker には materialize 済みの `String` が `set_system_prompt` で渡されるだけ。

### エラー処理

- 構文エラー → `Pod::from_manifest` 内のテンプレート parse で検出 → `PodError::InvalidSystemPromptTemplate` で起動失敗。
- render エラー（未定義変数など）→ first turn 開始時に `ensure_system_prompt_materialized` で検出 → `PodError::SystemPromptRender` で起動失敗。
- フォールバックは用意しない。fail-fast で統一する。

## 完了条件

- マニフェストに書いたシステムプロンプトがテンプレートとして解釈され、組み込み変数（date / cwd / scope / tools など）が first turn 開始時に展開されて LLM への system メッセージに反映される。
- compact を挟んでもシステムプロンプトが再評価されないことをテストで担保する。
- 外部ファイル系の変数（AGENTS.md など）は別チケットで供給するため、本チケットでは「変数として受け取れる器（空の `files` Map）」までを用意する。

## 範囲外

- AGENTS.md の読み取り自体は別チケット（`agents-md-ingestion.md`）で扱う。
- ユーザ単位の共通設定ファイル（Insomnia 独自の user config）は本チケットのスコープ外。

## 付随する変更

`SessionStartState.system_prompt` は materialize 後の値で埋まる必要があるため、`create_session` の呼び出しを `Pod::from_manifest` から `Pod::ensure_session_head` に寄せる（= session 作成そのものも遅延する）。これは hook / system_prompt と同じ ensure_\* パターンに揃える変更で、本チケットの一環として行う。
