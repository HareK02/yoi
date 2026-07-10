Worker filesystem authority の明示境界を実装・レビュー・merge・検証した。

実装内容:
- `WorkerFilesystemAuthority::{None, Local(LocalWorkingDirectory)}` を追加。
- `LocalWorkingDirectory` は filesystem authority `root` と default `cwd` を分離して保持する。
- Worker-level `cwd: PathBuf` field と `worker.cwd()` accessor を削除。
- core filesystem/Bash tool registration は local filesystem authority がある場合だけ行うようにした。
- No-workdir Worker では `Read` / `Write` / `Edit` / `Glob` / `Grep` / `Bash` を construct/register/model-visible にしない。
- Embedded no-workdir Worker spawn は Runtime/process cwd fallback ではなく `WorkerFilesystemAuthority::None` を明示的に渡す。
- Ticket / worker-spawn / peer-discovery など cwd 依存箇所は local filesystem authority 必須に整理し、workspace root fallback を authority として使わないようにした。
- tests で no-workdir tool omission、no cwd fallback、local/materialized Worker の filesystem/Bash behavior を確認。

Review:
- Reviewer approved with no blockers。
- Evidence included explicit authority type, no Worker cwd field/accessor, filesystem/Bash registration gate, embedded Runtime authority construction, and tests。
- Non-blocking note: workspace-context surfaces such as memory/MCP/resident prompt loading can still be backed by `workspace_root` when enabled; this is outside the focused core filesystem/Bash authority gate and may become follow-up if the invariant expands to “no local file reads of any kind”。

Merge / validation:
- Merge commit: `c29e9135 merge: worker filesystem authority`。
- Final validation passed:
  - `rg "worker\\.cwd\\(" crates || true`
  - `rg "pub fn cwd\\(" crates/worker/src/worker.rs || true`
  - `git diff --check`
  - `cargo test -p worker --lib --tests`
  - `cargo test -p worker-runtime --features ws-server,fs-store`
  - `cargo test -p yoi-workspace-server --lib`
  - `cargo check -p yoi`
  - `cd web/workspace && deno task check`
  - `cd web/workspace && deno task test`
  - `yoi ticket doctor`
  - `nix build .#yoi --no-link`
- Validation log: `/run/user/1000/yoi/yoi-orchestrator/bash-output/worker-fs-authority-final-validation-1783723253.txt`