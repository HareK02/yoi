Knowledge support removal を実装・レビュー・merge・検証した。

実装内容:
- active `KnowledgeQuery` tool registration/schema exposure を削除。
- Memory tools から `kind = knowledge` support と descriptions を削除。
- Memory crate の Knowledge frontmatter/schema/lint/query/resident-context handling を削除。
- Worker resident Knowledge injection と Knowledge completion state を削除。
- protocol / TUI / web の active `#<slug>` Knowledge reference/completion path を削除。
- Web Console の `#` completion sigil を削除し、`#...` は completion token ではなく plain/unsupported text になった。
- old persisted `kind: "knowledge"` SystemItem は `LegacyKnowledgeIgnored` として bounded compatibility にし、model context へ replay しない。
- `.yoi/knowledge/` は active workspace authority ではなく ignored/manual archive material として docs 更新。
- prompts/docs/tool descriptions から active Knowledge guidance を削除。
- Memory summary/decision/request behavior は維持。
- Agent Skills support はこの Ticket では実装していない。

Review:
- 初回 review は model-visible Ticket tool description と active design docs の stale Knowledge guidance で `request_changes`。
- 2回目 review は Web Console `#` completion が active のまま残っていた blocker で `request_changes`。
- 3回目 review は docs/report に active Knowledge wording が残っていた blocker で `request_changes`。
- `9f527f5e fix: remove stale report knowledge wording` 後の focused re-review は `approve`。

Merge / validation:
- Merge commit: `f279eb11 merge: remove knowledge support`。
- Final validation passed:
  - Focused Knowledge removal grep with only explicit legacy/manual `.yoi/knowledge` note remaining。
  - `git diff --check`
  - `cargo test -p memory --lib --tests`
  - `cargo test -p worker --lib --tests`
  - `cargo test -p session-store --lib --tests`
  - `cargo test -p tui --lib --tests`
  - `cargo test -p yoi --tests`
  - `cd web/workspace && deno task check`
  - `cd web/workspace && deno task test`
  - `cargo check -p yoi`
  - `yoi ticket doctor`
  - `nix build .#yoi --no-link`
- Validation log: `/run/user/1000/yoi/yoi-orchestrator/bash-output/knowledge-removal-final-validation-1784156249.txt`

Cleanup:
- Implementation worktree/branch cleanup will be performed after close commit。
- Per user instruction, `StopPod` is not used。