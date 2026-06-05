# Delegation intent: Task tools built-in feature module extraction

## Intent

Extract the Task tool group from the inline `pod::feature` registry implementation into a clean built-in internal feature module. This is a trial of the contribution-only built-in module path, not external plugin loading and not the external-plugin authority model.

## Worktree / branch

Use the prepared task name:

- worktree: `/home/hare/Projects/yoi/.worktree/task-tools-builtin-module`
- branch: `work/task-tools-builtin-module`

## Requirements

- Move the concrete Task feature implementation out of the generic registry/types file.
  - Current implementation is in `crates/pod/src/feature.rs` under `pub mod builtin`.
  - Candidate target: `crates/pod/src/feature/builtin/task.rs` or equivalent.
  - If converting `feature.rs` into a module directory is too large, use a small adjacent module with a clean public boundary, but keep the registry/types file focused on generic feature machinery.
- Expose a clear construction function such as:
  - `task_tools_feature(task_store: tools::TaskStore) -> impl FeatureModule`
- Preserve TaskStore sharing semantics.
  - The Pod host still creates/owns the Pod-lifetime/session `TaskStore`.
  - The Task feature receives the handle from the host constructor.
  - No TaskStore persistence/lifecycle change.
- Preserve exact tool behavior and model-visible metadata for:
  - `TaskCreate`
  - `TaskUpdate`
  - `TaskGet`
  - `TaskList`
- Ensure the Task feature descriptor declares exactly those four tools and requests no host authorities.
- Preserve descriptor-approved contribution reconciliation and once-materialized tool identity behavior.
- Keep normal ToolRegistry / PreToolCall permission behavior.
- Update imports/call sites so `Controller` uses the extracted built-in module.
- Add/update focused tests that prove Task tools install through the extracted built-in feature module and require no host authorities.
- Keep code comments/test names clear that this is a built-in/internal feature module reference pattern, not external plugin loading.

## Important boundary

Do not solve the broader authority-model split in this ticket. That is tracked separately by `feature-api-authority-separation`.

For this ticket:

- requested authorities for Task tools should stay empty;
- do not add or remove host authorities;
- do not reintroduce contribution-authority variants such as `ContributeTool`, `RunBackgroundTask`, or `ProvideService`;
- do not implement descriptor/package approval lock files.

## Non-goals

- External plugin loading.
- Plugin package format.
- WASM/runtime sandboxing.
- WorkItem or MCP integration.
- Moving Memory or Pod management.
- Reworking all built-in tool groups.
- Changing Task tool names, schemas, descriptions, outputs, or task reminder/compaction behavior.

## Suggested files

- `crates/pod/src/feature.rs`
- `crates/pod/src/lib.rs`
- `crates/pod/src/controller.rs`
- `crates/tools/src/task.rs`
- `crates/tools/src/lib.rs`
- existing feature tests in `crates/pod/src/feature.rs`

## Validation

Run at least:

- focused Task feature tests added/updated
- `cargo test -p pod --lib feature::tests --no-fail-fast` or equivalent focused feature tests
- `cargo test -p pod --lib`
- `cargo test -p llm-worker --lib`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `./tickets.sh doctor`
- `git diff --check`

Run `nix build .#yoi` if feasible.

## Completion report

Report:

- worktree path / branch
- commit hash
- changed files
- new module path / public constructor
- evidence Task feature descriptor has exactly four tools and no requested authorities
- behavior preservation notes
- tests/validation results
- unresolved risks/follow-ups
- whether ready for external review
