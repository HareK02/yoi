Merged and closed.

Implementation:
- Added explicit Profile/resolved Manifest `feature` configuration for Task, Memory, Web, Pods, Ticket, and Ticket orchestration tool surfaces.
- Disabled features omit tools from the Worker tool schema instead of registering them and denying later.
- Core filesystem/process tools remain outside this feature grouping and continue to be controlled by scope/policy.
- Ticket lifecycle access and Ticket orchestration surfaces are separable.
- Web, Memory, Ticket, and Pod tools retain their existing fail-closed / authority / scope checks when enabled.
- Project role profiles now set explicit feature defaults:
  - Orchestrator: Ticket lifecycle, Ticket orchestration, and Pods enabled; Task disabled.
  - Intake: basic Ticket enabled; Ticket orchestration, Pods, and Task disabled.
  - Coder/Reviewer/Companion: Ticket orchestration, Pods, and Task disabled; Ticket disabled in the current chosen defaults.

Commits:
- `f0f6cc9 feat: gate built-in tools by profile features`
- `2fd37af fix: align pod feature flag naming`
- `507863f fix: lock project role feature surfaces`
- `656048a test: cover project role feature profiles`
- merge: `c71a272 merge: gate tool surfaces by profile features`

Review:
- Earlier reviews requested `feature.pods` naming, project role Task-disable defaults, and actual project role profile coverage.
- Final review approved after `656048a`; merge-ready with only small residual E2E coverage risk noted.

Post-merge validation:
- `cargo test -p manifest actual_project_role_profiles_resolve_explicit_feature_defaults --lib`
- `cargo test -p manifest feature --lib`
- `cargo test -p pod project_role_tool_surfaces_keep_task_disabled_and_pods_role_scoped --test controller_test`
- `cargo test -p pod feature --tests`
- `cargo test -p tools --test integration`
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `cargo check --workspace`
- `nix build .#yoi`