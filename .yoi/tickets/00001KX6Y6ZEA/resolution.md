Worker workspace identity / WorkspaceBackend boundary を実装・レビュー・merge・検証した。

実装内容:
- Worker-held `workspace_root: PathBuf` identity を削除し、path-free `WorkerWorkspaceContext`, `WorkspaceId`, `WorkspaceClient` に置き換えた。
- Worker は `workspace_context` と `filesystem_authority` を分離して保持し、local filesystem authority は `WorkerFilesystemAuthority::{None, Local(LocalWorkingDirectory)}` に閉じ込めた。
- Worker constructors / restore / spawn paths を workspace context と filesystem authority の分離形に更新した。
- Runtime 側に `RuntimeWorkspaceBackendRef` を追加し、host-owned local backend binding から narrow Worker workspace context を注入するようにした。
- `workspace_id/client` が存在しても local filesystem authority を意味しないようにした。
- no-filesystem Workers では memory/workflow local layouts を workspace path fallback せず unavailable/fail-closed にした。
- legacy metadata `workspace_root` は compatibility/local filesystem hint として、`WorkerFilesystemAuthority::Local` 由来の場合だけ扱うようにした。
- tests で path-free workspace identity/client と local cwd/root の分離、filesystem authority なしの workspace client、local path-returning Worker workspace accessor 不在、constructor/restore/spawn behavior を確認した。

Review:
- Reviewer approved with no blockers。
- Evidence included path-free Worker context, no Worker `workspace_root()` accessor, Runtime/host backend materialization boundary, workspace-client-without-filesystem tests, fail-closed local workspace features, and legacy metadata confinement。
- Non-blocking note: `set_active_with_workspace_context` preserves existing metadata fields when passed `None`; future no-workspace reuse of an existing Pod name may want explicit clearing semantics to avoid stale UI hints。

Merge / validation:
- Merge commit: `391f11fc merge: workspace backend worker context`。
- Final validation passed:
  - `rg -n 'workspace_root' crates/worker crates/worker-runtime crates/client crates/workspace-server || true`
  - `rg -n 'worker\\.workspace_root|pub fn workspace_root|fn workspace_root' crates/worker crates/worker-runtime crates/client crates/workspace-server || true`
  - `git diff --check`
  - `cargo test -p worker --lib --tests`
  - `cargo test -p worker-runtime --features ws-server,fs-store`
  - `cargo test -p yoi-workspace-server --lib`
  - `cargo check -p yoi`
  - `cd web/workspace && deno task check`
  - `cd web/workspace && deno task test`
  - `yoi ticket doctor`
  - `nix build .#yoi --no-link`
- Validation log: `/run/user/1000/yoi/yoi-orchestrator/bash-output/workspace-backend-final-validation-1783732052.txt`