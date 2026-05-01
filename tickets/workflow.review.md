# Review: Workflow 実装

## 前提・要件の確認

- **Workflow loader / 検証**:
  - `<workspace>/.insomnia/memory/workflow/*.md` のスキャン → `crates/memory/src/workflow.rs:120-180`（`load_workflows`）。OK。
  - frontmatter 必須欠落・型不一致は hard error → `WorkflowLoadError::Frontmatter` 経由で `PodError::WorkflowLoad` まで持ち上がる（`crates/pod/src/pod.rs:2310-2315`）。OK。
  - slug とファイル名の不一致は `Slug::parse(stem)` で hard error 化 → `crates/memory/src/workflow.rs:154-159`。`Slug::parse` の kebab-case 強制が一致検証を兼ねている。OK。
  - 未知フィールドは `tracing::warn!` → `warn_unknown_workflow_fields`（`crates/memory/src/workflow.rs:218-238`）。OK。
  - 重複 slug は最初を採用 + warn → `crates/memory/src/workflow.rs:148-152`。OK。

- **`/<slug>` 呼び出し経路**:
  - `Segment::WorkflowInvoke` を `Pod::resolve_workflow_invocations` で resolve → `crates/pod/src/pod.rs:712-740`。
  - 解決失敗（未登録 / `user_invocable: false`）は Worker に届ける前に `Event::Error { code: InvalidRequest }` で返す → `crates/pod/src/controller.rs:366-372`、`Pod::validate_workflow_invocations`（`pod.rs:746-769`）。OK。
  - `requires` は `<workspace>/.insomnia/knowledge/<slug>.md` を `WorkspaceLayout::knowledge_path` 経由で直接読み、`Item::system_message` として Workflow 本文の前に積む（`crates/pod/src/workflow/mod.rs:90-130`）。ticket の「slug 完全一致経路で取得」は満たす。
  - Markdown 本文をそのまま利用、DSL 化なし。OK。

- **`model_invokation` 注入**:
  - `WorkflowRegistry::resident_entries` → `Pod::system_prompt` で `resident_workflow_slice` として `SystemPromptContext` に注入し、`resident_workflows_section` テンプレートで `## Resident workflows` 節を末尾に追加する（`crates/pod/src/pod.rs:592-612`、`crates/pod/src/prompt/system.rs:227-240`、`resources/prompts/internal.toml:47-54`）。OK。
  - description 上限 1024 chars（`WORKFLOW_DESCRIPTION_HARD_CAP`）を `model_invokation: true` の場合のみロード時に強制 → `crates/memory/src/workflow.rs:170-179`。Knowledge と同じポリシーで一貫。OK。
  - Phase 2 prompt には入れない: `set_resident_knowledge_injection(false)` 系のフラグで `inject_resident_knowledge` を落とすと workflow 注入も同時に無効化される（`pod.rs:589-598`）。OK。

- **`user_invocable: false` の挙動**:
  - 補完候補から除外 → `WorkflowRegistry::list_user_invocable` は `record.user_invocable` で filter（`crates/memory/src/workflow.rs:67-73`）。`shared_state.list_workflow_completions` が IPC `ListCompletions` に流れる（`crates/pod/src/ipc/server.rs:106-115`）。OK。
  - 明示呼び出しはエラー → `validate_workflow_invocations` が `NotUserInvocable` を返し `InvalidRequest` で client に返却。OK。

- **Linter ルール**:
  - 汎用 Write/Edit に対する `memory/workflow/` deny: `deny_write_rules` が `memory_dir()` を recursive deny するので `memory/workflow/` も covered（`crates/memory/src/scope.rs:19-24`）。OK。
  - memory tool 自身の workflow 書き込み禁止: `RecordKind::Workflow` を `Linter::lint` 冒頭で `LintError::WorkflowWriteForbidden` として弾く（`crates/memory/src/linter/mod.rs:101-105`）。`MemoryToolKind` enum に `Workflow` を含めないことで write/edit tool 自体が deserialize 段階で拒否（`crates/memory/src/tool/mod.rs:26-27`、`tool/write.rs:236-247`、`tool/edit.rs:242-253`）。OK。
  - consolidation の write tool schema に `workflow` カテゴリは含まれず（`crates/memory/src/consolidate/`、`extract/` に workflow への書き込み経路なし）。OK。
  - `lint_workflow` で frontmatter 検証 + `requires` 整合性チェック（`crates/memory/src/linter/mod.rs:250-280`）。OK。

- **単体テスト**:
  - frontmatter 検証の正常系: `loads_valid_workflow_with_default_flags` / 異常系: `invalid_filename_is_hard_error`、`missing_description_is_hard_error`（`crates/memory/src/workflow.rs` テスト）。
  - `requires` 解決: `resolves_requires_before_workflow_body`、`missing_required_knowledge_errors`（`crates/pod/src/workflow/mod.rs` テスト）。
  - フラグ別の挙動: `model_invokation_uses_typo_field`、`user_invocable_false_errors`。
  - すべてパス確認済み（`cargo test -p memory --lib workflow`、`cargo test -p pod --lib workflow`）。OK。

完了条件はすべて満たされている。

## アーキテクチャ・スコープ

- 低レベル基盤の `memory` クレートに loader / registry / linter を置き、Pod 側は resolver と registry の保持に留める。`llm-worker` 直下には触らず層分けは保たれている。OK（`feedback_llm_worker_scope` の方針に整合）。
- 新規モジュールはいずれもファイル名のみで `insomnia-` プレフィックスなし。OK。
- `crates/memory/src/workflow.rs` を新設し、schema/linter は既存ファイルへの最小追記。`pod.rs` への注入は既存の resident knowledge 経路を素直に拡張する形で、過剰な抽象化はない。`format_resident_entries` を `(slug, description)` のイテレータで共通化したのも妥当（`prompt/system.rs:255-285`）。
- `WorkflowResolveError` は `crates/pod/src/workflow/mod.rs` 内に閉じ、`PodError::WorkflowResolve` で `From` 実装を介してチェイン。Pod 公開 API は `validate_workflow_invocations` / `workflow_completions` の 2 つのみで surface も最小。OK。
- 依存追加は Cargo.lock の差分にあるが新規 crate 依存は導入されていない（既存 `memory`/`tracing`/`thiserror` で完結）。`cargo add` 違反なし。
- スコープ外の変更は無し。本ブランチの merge-base は `31d9b9b` で、`433ee0f` 1 コミットのみ。develop 直近の `00755cf "manifestで一部値のzeroの扱いを変更"` は merge-base より後の develop 側コミットであり、3-way merge で develop に残る（本ブランチは触れていない）。

## 指摘事項

### 対応済み（追加修正）

- **`lint_workflow` の description hard cap 検証** — loader 側 (`workflow.rs:170-179`) と挙動を揃えるため `Linter::lint_workflow` (`linter/mod.rs:250-`) で `model_invokation: true` の 1024 chars 上限を検証するよう追加。テスト 2 件 (`workflow_lint_flags_long_description_when_model_invokation` / `workflow_lint_allows_long_description_when_not_model_invokation`) を追加。
- **`resident_workflows_section` の文言** — `Invoke one with /<slug>` は LLM 側に invoke 経路がなく、かつ `model_invokation: true && user_invocable: false` の組み合わせ（resident に出るが `/<slug>` 拒否）が存在するため二重に誤誘導だった。`When a user request matches one, follow its procedure as authoritative instead of improvising. User-invocable workflows can additionally be triggered by the user typing /<slug>; you cannot invoke any of them yourself.` に書き換え（user_invocable に依存しない汎用形）。

### Non-blocking / Follow-up

- **Workflow body の `#<slug>` リファレンス** — Markdown 本文に `#<slug>` を書いても展開されない（resolver は `requires` フロントマターのみ参照）。`requires` で代用する設計だが、Workflow 著者が混乱しないよう将来の Workflow Linter で「本文中の `#<slug>` が `requires` に登録されているか」のチェックを追加する余地あり。

### Nits

- `crates/memory/src/workflow.rs:218-238` の `warn_unknown_workflow_fields` は許可フィールドの一覧をハードコードしており、`WorkflowFrontmatter` 側にフィールドを追加した際の同期忘れリスクがある。serde の `deny_unknown_fields` 相当を別パスで模しているとはいえ、配列定数を `pub const ALLOWED_FIELDS: &[&str]` のように schema 側に置いて両方から参照する方が安全。
- `WorkflowFrontmatter::updated_at` を `Option` 化して `epoch()` フォールバックする実装（`schema/workflow.rs:35-40`）は MVP 用途として合理的だが、`Frontmatter::updated_at` が「実時刻 or epoch」のどちらを返すかが呼び出し側で見えない。利用箇所（`size::check_body` 経由）に影響しない範囲だが、トレイトのコメントに「workflow は epoch を返し得る」旨を一言入れておくと混乱を避けられる。
- `WorkflowResolveError::InvalidSlug` は `validate_workflow_invocations` 経由でしか発火しないが、ユーザー入力の段階で TUI 側 `Atom::WorkflowInvoke` が同じ `Slug::parse` を通っているはず（`crates/tui/src/input.rs`）。重複検証は害ではないが冗長。

## 判断

**Approve** — チケット要件は完全に満たされ、テストも妥当。スコープ外の変更も無し。指摘はすべて follow-up / nit レベル。
