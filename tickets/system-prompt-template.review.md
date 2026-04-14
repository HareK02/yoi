# Review: system-prompt-template

## 前提・要件の確認

チケットの完了条件と要件をひとつずつ実装と照合した。

### 評価モデル ✓

- `Pod` が `system_prompt_template: Option<SystemPromptTemplate>` を保持し、`Pod::from_manifest` でパース、`Pod::run`/`Pod::resume` 冒頭の `ensure_system_prompt_materialized` が `Option::take()` で1回だけ materialize する — `pod.rs:385-412`。
- 既存の `ensure_interceptor_installed` の直後に置かれており、hook と対称な ensure_\* パターンに揃っている。
- materialize 後は `worker.set_system_prompt(rendered)` で Worker の既存フィールドに流し込む。以降の compact 経由でも template フィールドは `None` のため再 render 不可 — 構造で担保されている。

### テンプレート構文・変数 ✓

- minijinja 2.19 を採用、`UndefinedBehavior::Strict`、エスケープは `{{ '{{' }}`、single-named template 方式。
- 組み込み変数の初期セット（`date` / `time` / `datetime` / `cwd` / `scope.{readable,writable,summary}` / `tools` / `files`）が `SystemPromptContext::to_minijinja_value` で全部供給されている — `system_prompt.rs:82-135`。
- `files` は常に空 BTreeMap で、`{% if files.x is defined %}` 構文で安全にガードできることをテストで確認（`system_prompt.rs:234`）。

### マニフェスト上の記法 ✓

- 既存 `WorkerManifest.system_prompt: Option<String>` をテンプレート文字列として解釈。新フィールドは増えていない。
- `apply_worker_manifest` では敢えて `system_prompt` を触らず、doc コメントで理由を明示（`pod.rs:829-834`）。

### 責務の分離 ✓

- テンプレート機構は `crates/pod/src/system_prompt.rs` に閉じ、`llm-worker` はテンプレートの存在を一切知らない。
- 唯一 llm-worker 側に入った変更は `ToolServerHandle::flush_pending` の `pub(crate) → pub` 化。これはテンプレート render 前にツール名を確定させるためで、Worker 層の機能性は変わらず、理由も doc で明示されている（`tool_server.rs:72-82`）。基盤原則への違反ではない。

### エラー処理 ✓

- 構文エラー → `PodError::InvalidSystemPromptTemplate`、render エラー → `PodError::SystemPromptRender`。両方とも `#[source]` 付きで `SystemPromptError` をラップ。fail-fast 一貫。

### 付随する変更 ✓

- `create_session` を `from_manifest` から外し、`ensure_session_head` 側で `create_session_with_id` を呼ぶように遅延化。`session-store` 側に `create_session_with_id` を新設 — ID を先に確定しつつ初回 log append を遅延するユースケース用、doc コメント付き。
- 結果として `SessionStartState.system_prompt` が materialize 後の値で埋まる。`session_start_state_captures_rendered_prompt` テストで log entry を直接検証。

### 完了条件 ✓

- マニフェスト → テンプレート解釈 → 組み込み変数展開 → LLM system message 反映の一連の流れが `materialise_on_first_turn_populates_worker` で確認されている。
- compact を挟んでもシステムプロンプトが再評価されないことを `compact_preserves_system_prompt` が直接検証。
- `files` は空 Map の器として用意済み（AGENTS.md 供給は別チケット）。

### テスト

- `crates/pod/src/system_prompt.rs` 単体: 7本（parse 成功/失敗、date/cwd/tools 置換、未定義変数、`{{` エスケープ、scope.summary、files 空）。
- `crates/pod/tests/system_prompt_template_test.rs` 統合: 7本（parse エラー、first run 前未 materialize、first turn で materialize、SessionStart capture、render エラー伝播、2 turn 間で一意、compact 越え保存）。
- `cargo test --workspace`: 323 passed / 0 failed。

## 指摘事項

いずれも blocking ではない。load-bearing な修正は無い。

### 1. `Pod::new` が `manifest.worker.system_prompt` を黙って無視する（軽微）

`Pod::new` は manifest を受け取るが template parse は行わず、`system_prompt_template: None` で初期化する。そのため `Pod::new` 経由で manifest 由来の system_prompt を効かせたい場合はテスト側で `SystemPromptTemplate::parse` + `set_system_prompt_template` を手動で呼ぶ必要がある。

現状 `Pod::new` は production からは使われておらず、用途はテストのみ（`controller_test.rs`, `system_prompt_template_test.rs`）。production 経路は `Pod::from_manifest` 一本で、そちらは正しく parse する。

- **判断**: 現状維持で OK。`Pod::new` は低レベルコンストラクタであり、manifest の解釈を完全に担う責務は持っていない（`apply_worker_manifest` 相当の処理も外から呼ぶ想定）。ただし doc コメントで「`system_prompt` template は Pod::new の対象外、必要なら `set_system_prompt_template` を呼べ」と一行書いておくと事故を防ぎやすい。

### 2. `flush_pending` の public 化は「Pod 側でツール名を先取りする」ため（情報共有）

Pod の `ensure_system_prompt_materialized` は Worker の lock() より前に走るので、その時点で pending factory はまだ materialize されていない可能性がある。そこで `worker.tool_server_handle().flush_pending()` を明示的に呼んでツール一覧を確定させている（`pod.rs:393`）。

flush_pending は冪等であり（実装上も空 Vec を drain するだけ）、Worker::lock() 側での二重呼び出しも問題ない。API 的には「force-materialise を higher layer が要求できる」という意味付けが明確なので、doc コメントがあれば十分。既に doc は追加済み。

- **判断**: 現状維持で OK。`llm-worker` を低レベル基盤に留める原則に照らしても、「明示的フラッシュ」という低レベル操作を公開するだけで、テンプレートの概念は漏れていない。

### 3. `SystemPromptTemplate` が単一テンプレート専用に `Environment<'static>` を抱える（美観）

`minijinja::Environment` はテンプレートの名前空間を持つが、ここでは固定名 `"system_prompt"` で1枚だけ登録している。代替としては `minijinja::Template::new` 相当のスタンドアロンパースもあるが、minijinja の public API は Environment 経由が主流で、コストは Arc 1 つ分。

- **判断**: 現状維持で OK。将来 filter / function を足したくなった時に Environment を使うほうが素直。

### 4. `render` 時の `Environment::get_template` が `Result` を返すことへの扱い（微細）

`SystemPromptTemplate::render` は `get_template` の失敗も `SystemPromptError::Render` にマップしているが、parse 時に必ず登録しているので実質到達しない経路。`expect` でも良いが、`.map_err` で統一されているのは無害。

- **判断**: 現状維持で OK。

### 5. `SystemPromptContext` の `tool_names: Vec<String>` を `&[String]` にできる余地（微細）

所有の必要性は無いので参照でも良い。ただし `ensure_system_prompt_materialized` が一時 Vec を作る以上、所有のほうが呼び出し側が楽。AGENTS.md 取り込み時に `files` を構築するコードと書き味を合わせる意味でも所有のままで良い。

- **判断**: 現状維持で OK。

## 結論

**accept**。チケットの要件を過不足なく満たしており、テストも 1:1 で要件項目を検証している。指摘事項はいずれも doc コメントの追記レベルで、必須ではない。
