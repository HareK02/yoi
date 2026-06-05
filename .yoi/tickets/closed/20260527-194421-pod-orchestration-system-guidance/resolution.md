Merged and completed.

Implementation:
- Added resource-backed Pod orchestration guidance at `resources/prompts/common/pod-orchestration.md`.
- Registered the guidance through the prompt catalog and internal prompt resources.
- Added conditional system prompt assembly based on registered Pod-management tool names.
- Guidance is included for Workers with Pod management tools and omitted otherwise.
- Guidance explicitly says Pod notifications are background signals handled at natural stopping points, that the parent does not need to keep a turn open solely to wait, and that agents should not use `sleep`/polling loops just to wait for Pod output.
- Guidance also preserves evidence-before-completion and no scheduler/authorization-bypass constraints.

Review:
- External reviewer approved with no blockers.

Validation after merge:
- `cargo test -p pod pod_orchestration` passed.
- `cargo test -p pod prompt::catalog` passed.
- `cargo fmt --check` passed.
- `./tickets.sh doctor` passed.
