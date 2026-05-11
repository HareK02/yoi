# memory / workflow の共通基盤を別 crate に切り出す

## 背景

`tickets/workflow-crate-extraction.md`（完了済 / git log 参照）で Workflow を `crates/memory/` から `crates/workflow/` に切り出した際、依存方向を「workflow → memory（`WorkspaceLayout` のみ）」に限定するため、本来共通であるべき型・関数を **両 crate にコピペで重複** させて済ませている。

具体的に重複しているもの:

- **`Slug` 型と `is_valid_slug`**: `crates/memory/src/slug.rs` と `crates/workflow/src/slug.rs` がエラー型（`LintError::InvalidSlug` / `WorkflowLintError::InvalidSlug`）以外完全に同じ。テストごと丸ごとコピー。
- **`split_frontmatter`**: `crates/memory/src/schema/common.rs` と `crates/workflow/src/schema.rs` に同等の実装。返すエラー型だけ違う。
- **YAML frontmatter の `MissingFrontmatter` / `MalformedFrontmatter` バリアント**: `LintError` と `WorkflowLintError` の両方に重複定義。
- **`Frontmatter` trait（`created_at` / `updated_at` の統一アクセス）**: 現状 memory 側だけにあり、workflow 側の `WorkflowFrontmatter` は同 trait を実装していない。共通 crate に出るなら、workflow 側でも揃えられる。

memory / workflow どちらも agent-skills 互換のスラグ規約と Markdown + YAML frontmatter の同一フォーマットを採用しているため、これらは設計上「両者が共有すべき同一の概念」であって、別物として持つ理由はない。`tickets/workflow-crate-extraction.md` も完了条件と直交する形で「共有が必要なら共通部分を別 crate（例: `crates/lint-common/`）に切る判断を行う」と前置きしており、抽出時にスキップした判断を本チケットで補う。

## 要件

### 新 crate の新設

memory / workflow 双方が依存する共通 crate を 1 つ立てる。crate 名は実装時に決める（候補: `lint-common`, `record-core`, `frontmatter` など）。memory / workflow より下層に位置し、両者が import する。

新 crate が持つもの:

- `Slug` 型 + `is_valid_slug`（agent-skills 互換規約）
- `split_frontmatter`（YAML frontmatter / Markdown body 分離）
- 上記に紐づく共通エラー型（`InvalidSlug` / `MissingFrontmatter` / `MalformedFrontmatter`）
- `Frontmatter` trait（`BODY_LIMIT` / `created_at` / `updated_at` のアクセサ）

### memory / workflow からの重複削除

- `crates/memory/src/slug.rs` と `crates/workflow/src/slug.rs` を削除し、新 crate の `Slug` を再 export または直接 import する形に書き換える
- `crates/memory/src/schema/common.rs` 内の `split_frontmatter` と `crates/workflow/src/schema.rs` 内の `split_frontmatter` を新 crate のものに統合
- `LintError` / `WorkflowLintError` の `InvalidSlug` / `MissingFrontmatter` / `MalformedFrontmatter` バリアントは、共通エラー型を `#[from]` で包む形に揃えるか、共通エラー型をそのまま使う形に切り替える（実装時に判断）
- `WorkflowFrontmatter` も共通 `Frontmatter` trait を実装するように揃える（`BODY_LIMIT` を 8000 で踏襲）

### 依存方向

- 新 crate は memory / workflow / その他に依存しない（純粋なドメイン型のみ）
- memory / workflow 双方が新 crate を import する
- workflow → memory の `WorkspaceLayout` 依存は維持（このチケットの対象外）

## 範囲外

- linter 本体の共通化（memory `Linter` と workflow `WorkflowLinter` の統合）
- `WorkspaceLayout` の memory crate からの切り出し
- `WorkflowFrontmatter` / `KnowledgeFrontmatter` 等のスキーマ変更
- agent-skills 互換規約自体の変更

## 完了条件

- 新 crate が `Slug` / `is_valid_slug` / `split_frontmatter` / 共通エラー型 / `Frontmatter` trait を提供している
- `crates/memory/src/slug.rs` と `crates/workflow/src/slug.rs` の重複コードが消えている（少なくとも一方からは）
- `split_frontmatter` の実装が 1 箇所に集約されている
- `WorkflowFrontmatter` が `Frontmatter` trait を実装している
- 既存テスト（memory / workflow / pod）が新構造で通る
- 循環依存が無い

## 参照

- 直前: `tickets/workflow-crate-extraction.md`（git log、`workflow-crate-extraction.review.md` で本件が見落とされた経緯あり）
- 関連: `tickets/internal-worker-workflow.md`（本チケット完了後に着手すると共通基盤が揃った状態で進められる）
