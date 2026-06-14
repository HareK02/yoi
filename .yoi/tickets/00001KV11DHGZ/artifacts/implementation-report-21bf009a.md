実装報告（Coder）

Commit:
- implementation: `21bf009a3f95978007468005982903c8d7cae9e7` (`feat: move profile scope to launch policy`)

変更ファイル:
- `resources/profiles/default.lua`
- `resources/profiles/companion.lua`
- `resources/profiles/intake.lua`
- `resources/profiles/orchestrator.lua`
- `resources/profiles/coder.lua`
- `resources/profiles/reviewer.lua`
- `crates/manifest/src/profile.rs`
- `crates/manifest/src/config.rs`
- `crates/pod/src/entrypoint.rs`
- `crates/pod/src/spawn/tool.rs`

実装内容:
- Builtin reusable Profiles から concrete filesystem `scope` / `delegation_scope` を削除した。
- Profile resolution の implicit default workspace-write scope を廃止し、Profiles without scope を empty scope として解決可能にした。
- Fresh profile launch の effective authority を `pod` entrypoint の launch policy で付与するようにした。
  - normal Companion/TUI/default profile launch: workspace write direct scope（`.worktree` write deny）+ workspace read / `.worktree` write delegation。
  - Ticket Orchestrator role launch: original workspace read direct scope + original workspace read / `<workspace>/.worktree` write delegation。root workspace write delegation は付与しない。
  - Ticket Intake/Reviewer role launch: workspace read direct scope。
  - Ticket Coder role launch: workspace write direct scope。
- Single-file `--manifest` mode は従来どおり concrete `scope.allow` を要求し、Profile launch policy と混同しないようにした。
- SpawnPod child scope replacement path は維持し、narrow Orchestrator delegation で Coder/Reviewer 用の root-read + implementation-worktree-write validation が成立することをテストした。
- Existing user Profile `scope` / `delegation_scope` compatibility は deprecated-compatible path として残した（Ticket の選択肢に沿って、builtin role launch authority は Profile scope に依存しない）。

Validation:
- `cargo test -p manifest` ✅
- Focused tests ✅
  - `cargo test -p manifest builtin_role_profiles_are_registered_and_resolve --quiet`
  - `cargo test -p manifest builtin_default_resolves_without_external_evaluator --quiet`
  - `cargo test -p manifest resolve_accepts_empty_scope_for_profile_launch_policy --quiet`
  - `cargo test -p pod --lib entrypoint::tests::normal_startup_uses_default_profile --quiet`
  - `cargo test -p pod --lib entrypoint::tests::orchestrator_profile_launch_gets_read_root_and_worktree_delegation_from_launch_policy --quiet`
  - `cargo test -p pod --lib spawn::tool::tests::orchestration_delegation_allows_root_read_and_worktree_writes_not_root_writes --quiet`
  - `cargo test -p pod --lib pod::pod_metadata_restore_manifest_tests::snapshot_preserves_saved_scope_over_current_manifest --quiet`
  - `cargo test -p client ticket_role --quiet`
- `cargo build -p yoi` ✅
- `cargo fmt --check` ✅
- `git diff --check` ✅
- `nix build .#yoi` not run: no Cargo.lock, packaging, or resource inclusion pattern changed.

Residual risks / notes:
- Full `cargo test -p pod --lib` was attempted and still has two prompt-text assertion failures (`worktree status, diff, and test results`) in prompt catalog/system tests; this diff did not touch prompt resources or those assertions. Focused scope/profile/spawn/restore tests passed.
- User Profile `scope` compatibility remains supported for now; future schema cleanup can remove or deprecate it explicitly if desired.
