Backend Worker/Workdir registry and link model を実装・レビュー・merge・検証した。

実装内容:
- Backend SQLite schema に `worker_registry`, `workdir_registry`, `worker_workdir_links` を追加。
- Worker registry / Workdir registry / Worker-Workdir link の typed store API と projection を追加。
- Backend 経由の Workdir 作成で Runtime materialization 前に Backend-managed pending row を作成。
- Backend / Runtime scoped Worker creation で Worker registry row、Workdir registry row、Worker-Workdir link を同期。
- Worker list は Runtime observation を同期したうえで Backend `worker_registry` / link authority から projection。
- Worker stop は canonical registry row を削除せず、Worker lifecycle と linked Workdir status を同期。
- Workdir list / launch options は Backend-managed rows を基準にし、`runtime_unmanaged` Workdirs は managed Browser list/options に混ぜず runtime/diagnostic projection で区別可能にした。
- `pinned` retention が通常 Runtime sync で `normal` に clobber されないよう保護。
- Browser-facing summaries に safe `management_kind` を追加し、raw Runtime materialized path を Backend registry / Browser response に保存・表示しない方針を維持。

Review:
- 初回 review は 5 blocker で `request_changes`。
- `8695089f fix: sync backend worker registry projections` で主要 blocker を修正。
- 2回目 review は unmanaged Workdir が managed list/options に混ざる blocker で `request_changes`。
- `239f93b0 fix: keep unmanaged workdirs out of managed lists` で残 blocker を修正。
- 最終 focused re-review は `approve`。

Merge / validation:
- Merge commit: `03652f82 merge: worker workdir registry`。
- Final validation passed:
  - `git diff --check`
  - `cargo test -p yoi-workspace-server --lib`
  - `cargo test -p worker-runtime --features ws-server,fs-store`
  - `cargo check -p yoi`
  - `cd web/workspace && deno task check`
  - `cd web/workspace && deno task test`
  - `yoi ticket doctor`
  - `nix build .#yoi --no-link`
- Validation log: `/run/user/1000/yoi/yoi-orchestrator/bash-output/worker-workdir-final-validation-1783704956.txt`

Non-blocking follow-up noted by reviewer:
- Some runtime detail/create response paths still return direct Runtime summaries with `management_kind: None`; list/options surfaces required by this Ticket are safe, but a future cleanup can project detail responses through Backend records for more uniform diagnostics。