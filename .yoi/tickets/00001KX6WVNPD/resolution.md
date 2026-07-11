Embedded no-workdir Worker authority policy を実装・レビュー・merge・検証した。

実装内容:
- Runtime launch metadata に `working_directory_required` を追加し、embedded Runtime のみ workdir optional として扱うようにした。
- Workspace/API Worker create path で、embedded Runtime では no-workdir Worker creation を許可し、non-embedded Runtime では no-workdir creation を typed diagnostic で拒否するようにした。
- Embedded no-workdir launch は Runtime へ working directory なしで渡され、merged `WorkerFilesystemAuthority` 境界により `WorkerFilesystemAuthority::None` になる。
- Workdir-present embedded Worker は materialized binding を `WorkerFilesystemAuthority::Local` として保持する既存 behavior を維持。
- Browser Worker creation UI で embedded Runtime の場合だけ “No working directory — guidance-only Worker; filesystem tools are disabled” を選べるようにした。
- No-workdir launch では relative cwd input を隠し、filesystem tools / Bash が disabled であることを明示。
- Tests で launch metadata、non-embedded no-workdir rejection、embedded no-workdir request construction、および existing runtime tests による no-workdir tool omission/no cwd fallback を確認。

Review:
- Reviewer approved with no blockers。
- Evidence included API optionality bounds, non-embedded rejection, authority construction path, no filesystem/Bash tool surface coverage, workdir-present behavior, and Browser labeling。

Merge / validation:
- Merge commit: `2f7b8094 merge: embedded no-workdir worker policy`。
- Final validation passed:
  - `rg 'worker\\.cwd\\(' . || true`
  - `git diff --check`
  - `cargo test -p worker-runtime --features ws-server,fs-store`
  - `cargo test -p yoi-workspace-server --lib`
  - `cargo check -p yoi`
  - `cd web/workspace && deno task check`
  - `cd web/workspace && deno task test`
  - `yoi ticket doctor`
  - `nix build .#yoi --no-link`
- Validation log: `/run/user/1000/yoi/yoi-orchestrator/bash-output/embedded-no-workdir-final-validation-1783728763.txt`