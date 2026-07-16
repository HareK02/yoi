Workflow tracking / Workflow resource / Workflow invocation machinery を削除・レビュー・merge・検証した。

実装内容:
- model-visible `ActiveWorkflowList`, `ActiveWorkflowComplete`, `ActiveWorkflowCancel` tools を削除。
- Worker active-workflow durable state、extension snapshotting、compaction/rehydration handling、prompt re-injection paths を削除。
- Workflow registry/resource loading と `/workflow-slug` invocation behavior を削除。
- `crates/workflow` crate と tracked `resources/workflows/*` resources を削除。
- `WorkflowInvoke` を protocol / TUI / web generated protocol surfaces から削除。
- slash workflow completion behavior と related web console completion surface を削除。
- `.yoi/workflow` memory/workspace authority と workflow usage-source handling を削除。
- resident workflow / workflow invocation wording を prompts/docs/config から削除。
- old persisted `kind: "workflow"` `SystemItem`s は `LegacyIgnored` として bounded diagnostic/ignore behavior のみにした。
- first-class Skills support はこの Ticket では実装していない。

Review:
- 初回 review は web/generated `workflow_invoke` surface、stale workflow role/test、docs wording で `request_changes`。
- 2回目 review は slash workflow completion path で `request_changes`。
- 3回目 review は TUI test fixture が legacy workflow SystemItem を active snapshot carrier として使っている blocker で `request_changes`。
- `bc48094d fix: update workflow-free tui tests` 後の focused re-review は `approve`。

Merge / validation:
- Merge commit: `2f260029 merge: remove workflow machinery`。
- Final validation passed:
  - `rg "ActiveWorkflow|active_workflow|Active workflow" . || true`
  - `rg "resources/workflows|\\.yoi/workflow|workflow invocation|Resident workflows|workflow_invoke" . || true`
  - `git diff --check`
  - `cargo test -p ticket`
  - `cargo test -p session-store --lib --tests`
  - `cargo test -p worker --lib --tests`
  - `cargo test -p tui --lib --tests`
  - `cargo test -p yoi --tests`
  - `cd web/workspace && deno task check`
  - `cd web/workspace && deno task test`
  - `cargo check -p yoi`
  - `yoi ticket doctor`
  - `nix build .#yoi --no-link`
- Validation log: `/run/user/1000/yoi/yoi-orchestrator/bash-output/workflow-removal-final-validation-1784151359.txt`

Cleanup:
- Implementation worktree/branch cleanup will be performed after close commit。
- Per user instruction, `StopPod` is not used。