# Review: Workflow crate extraction

## 対象

- Ticket: `tickets/workflow-crate-extraction.md`
- Branch: `workflow-crate-extraction`
- Reviewed commit: `7db4146 refactor: extract workflow crate`

## 確認内容

- `crates/workflow/` が workspace member として追加され、Workflow loader / registry / frontmatter schema / Skill ingestion / human-edit linter / workflow scope deny helper を持つ。
- `WorkflowRecord`, `WorkflowRegistry`, `WorkflowSource`, `WorkflowLoadError`, `ResidentWorkflowEntry`, `ShadowedSkill`, `WorkflowFrontmatter`, `SkillRecord`, `load_workflows`, `load_skills_from_dir` は `workflow` crate から export される。
- workflow crate は memory crate へは `WorkspaceLayout` のためだけに依存している。Workflow 用 slug / lint error / frontmatter split は workflow crate 側に持ち、memory crate の workflow-specific API へ依存していない。
- memory crate から `workflow.rs`, `skill.rs`, `schema/workflow.rs` と re-export が削除されている。
- memory linter から `lint_workflow` が削除され、human-edit workflow linter は `workflow::WorkflowLinter` に移っている。
- memory scope deny は memory / knowledge のみを表明し、workflow crate が `.insomnia/workflow/` の deny を表明する。Pod 起動時に両方を合成している。
- pod 側の Workflow registry / resident workflow / Skill ingestion / system prompt rendering / workflow resolver は `workflow_crate` を直接 import する形に更新されている。
- ticket 範囲外の Workflow schema 変更、resolver 機能変更、bundled default Workflow、reasoning / prune 変更は入っていない。

## 検証

- `cargo test -p workflow -p memory -p pod` passed
- `cargo fmt --check` failed due existing unrelated rustfmt diffs in `llm-worker`, `session-store`, and `tui`; this ticket's changed files were formatted directly with `rustfmt --edition 2024`.

## 判断

Approve.

Workflow domain code is now isolated in `crates/workflow/`, memory no longer re-exports or owns Workflow / Skill modules, and scope deny composition is explicit between memory and workflow. The remaining dependency on memory is limited to `WorkspaceLayout`, matching the ticket's allowed design point.
