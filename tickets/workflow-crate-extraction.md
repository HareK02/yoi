# Workflow を memory crate から独立させる

## 背景

`tickets/workflow-directory-layout.md` で Workflow の物理配置を `.insomnia/workflow/` に分離した。これにより Workflow は概念上 memory state（session-derived / generated）と別物として整理されたが、ソースコード上は依然として `crates/memory/` 配下に同居している:

- `crates/memory/src/workflow.rs`（`WorkflowRecord` / `WorkflowRegistry` / `WorkflowSource` / `load_workflows` / `WorkflowLoadError` / `WORKFLOW_DESCRIPTION_HARD_CAP` / `ResidentWorkflowEntry` / `ShadowedSkill`）
- `crates/memory/src/schema/workflow.rs`（`WorkflowFrontmatter`）
- `crates/memory/src/skill.rs`（Skill → Workflow projection）
- `crates/memory/src/linter/mod.rs::lint_workflow`（人間編集向けの workflow linter）
- `crates/memory/src/error.rs::LintError::WorkflowWriteForbidden`

memory crate のドメインは「decisions / requests / summary / knowledge / staging / consolidation」に絞り、Workflow は独立した crate に出す。`tickets/internal-worker-workflow.md` で内部 Worker の Workflow 化が予定されており、bundled default や `internal_role` 追加の置き場として独立 crate がある方が自然。

## 要件

### crate の分離

`crates/workflow/` を新設し、上記の Workflow 関連型 / 関数 / スキーマ / Skill projection / human-edit linter を移す。

- 新 crate からは memory crate に依存しないか、`WorkspaceLayout` 経由で薄く依存するに留める
- `crates/memory/` から workflow 関連の `pub use` 再エクスポートは削除（呼び出し側が新 crate を直接 import する）
- Workflow 用の linter は memory crate の `Linter` を共有しないでよい場合は単独で持つ。共有が必要なら共通部分を別 crate（例: `crates/lint-common/`）に切る判断を行う

### `WorkspaceLayout` の扱い

`workflow_dir()` / `workflow_path()` が memory crate に残るかは設計判断:

- memory crate に残し、workflow crate がそれを利用する形でよい
- 別 crate（例: `crates/workspace-layout/`）に切り出す場合は memory / workflow 両方が参照する形にする

どちらでもよいが、結果として循環依存を生まないこと。

### 既存 use site の更新

- `crates/pod/`（`pod.rs` / `prompt/system.rs` / `workflow/mod.rs`）
- `crates/tui/`
- その他 `memory::Workflow*` を import している箇所

これらが新 crate を import する形に書き換わる。

### Skill ingestion の所属

`SKILL.md` パーサと `WorkflowRecord` への projection は workflow crate に同居する。Skills は外部入力だが最終的に Workflow registry に流れるので、workflow crate を窓口にする方がレイヤとして自然。

### scope deny の整理

`crates/memory/src/scope.rs::deny_write_rules` は memory / knowledge / workflow の 3 ディレクトリを deny している。workflow crate 側で `.insomnia/workflow/` の deny を表明し、Pod 起動時に両方を合成する形にするか、あるいは scope deny は呼び出し側（pod）で集約する形に再設計する。

## 範囲外

- Workflow の機能変更（frontmatter schema 変更、resolver 改修等）
- bundled default Workflow 機構（`tickets/internal-worker-workflow.md` の対象）
- memory crate 内部の他モジュール再編

## 完了条件

- `crates/workflow/` crate が独立して存在し、`WorkflowRecord` / `WorkflowRegistry` / `load_workflows` / `WorkflowFrontmatter` / Skill projection / human-edit linter がそこに住む
- memory crate に workflow / skill 関連のソースが残っていない（reexport も無し）
- 既存テストが新構造で通る
- 既存呼び出し側（pod / tui 等）が新 crate を import する形に更新されている
- scope deny が memory / workflow を矛盾なく合成できる構成になっている

## 参照

- 直前: `tickets/workflow-directory-layout.md`（git log）
- 後続: `tickets/internal-worker-workflow.md`
- 関連: `docs/plan/workflow.md`

## レビュー状態

- `7db4146 refactor: extract workflow crate` を review 済み。結果は `tickets/workflow-crate-extraction.review.md`。
- 判断: approve / merge 可。
