Agent Skills support を実装・レビュー・merge・検証した。

実装内容:
- first-class Skill schema/types と Workspace HTTP client support を追加。
- Workspace backend に Skill discovery / lint / catalog / detail / activation を追加。
- Workspace API endpoints を追加:
  - `GET /api/w/{workspace_id}/skills`
  - `GET /api/w/{workspace_id}/skills/lint`
  - `GET /api/w/{workspace_id}/skills/{name}`
  - `GET /api/w/{workspace_id}/skills/{name}/activate`
- `yoi-workspace-server skills list|lint|show` CLI surface を追加。
- Web Workspace API helpers/types を追加。
- builtin Skill resource `resources/skills/agent-skills/SKILL.md` を追加。
- Worker Skill activation method を追加し、`WorkspaceClient::Http` 経由で Skill activation を取得し、`SystemItem::SkillActivation` を commit し、Skill body を engine history に append するようにした。
- Prompt guidance を更新し、Skills は procedural LLM guidance であり external state authority ではないことを明示。

Skill behavior:
- Workspace Skills are read from `.yoi/skills/<skill-name>/SKILL.md`。
- Lint validates required `name` / `description`, parent-dir match, name length/pattern, description bounds, optional `license` / `compatibility` / string-map `metadata`。
- Unknown frontmatter keys are lint errors。
- Workflow/projection/invocation-shaped keys such as `model_invokation`, `user_invocable`, `graph`, `invocation` are explicitly rejected with `unsupported_workflow_frontmatter_field`。
- `allowed-tools` is parsed/diagnosed as experimental ignored/non-authoritative metadata。
- Builtin and workspace Skills are loaded deterministically; workspace Skills override builtin Skills with path-free provenance。
- Catalog responses contain lightweight metadata only; detail/activation returns full `SKILL.md` body。
- References/assets/scripts are Skill-relative and non-executable/non-authoritative; no raw absolute paths are exposed。
- No `/skill-name` syntax, Workflow compatibility path, or Knowledge active support was added。

Review:
- Initial review requested changes for silently accepted unsupported Workflow frontmatter and missing Worker activation/history coverage。
- `4ddfccee fix: reject unsupported skill fields` added unsupported-key diagnostics/regression tests and Worker activation/history tests。
- Focused re-review approved with no blockers。
- Non-blocking note: detail/activation for invalid Skills returns not-found while catalog/lint carries diagnostics; acceptable for this initial authority model。

Merge / validation:
- Merge commit: `1611e04d merge: agent skills support`。
- Final validation passed:
  - Regression grep for removed Workflow/Knowledge active surfaces。
  - `git diff --check`
  - `cargo test -p yoi-workspace-server --lib`
  - `cargo test -p worker --lib --tests`
  - `cargo test -p yoi --tests`
  - `cd web/workspace && deno task check`
  - `cd web/workspace && deno task test`
  - `cargo check -p yoi-workspace-server`
  - `cargo check -p yoi`
  - `yoi ticket doctor`
  - `nix build .#yoi --no-link`
- Validation log: `/run/user/1000/yoi/yoi-orchestrator/bash-output/agent-skills-final-validation-1784160679.txt`

Cleanup:
- Implementation worktree/branch cleanup will be performed after close commit。
- Per user instruction, `StopPod` is not used。