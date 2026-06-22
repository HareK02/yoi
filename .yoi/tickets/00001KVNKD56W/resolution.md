Workspace DB canonical schema v0 design を `schema-v0.md` artifact と workspace-server SQLite bootstrap/migrations に整合させ、Orchestrator worktree の `orchestration` branch に統合した。

主な成果:
- SQLite schema version を `2` に進め、canonical schema v0 を実装。
- Typed v0 tables を追加:
  - `tickets`
  - `ticket_events`
  - `ticket_relations`
  - `objectives`
  - `objective_ticket_links`
  - `repositories`
  - `ticket_targets`
  - `ticket_target_paths`
  - `ticket_worker_links`
  - `artifacts`
  - `audit_events`
- Canonical v0 から top-level `runs`, `hosts`, `workers`, `actors`, validation/CI result tables を除外。
- Generic `metadata_json`, `payload_json`, `diagnostics_json` のような catch-all payload columns を canonical v0 tables に追加しない方針を維持。
- `/api/runs` と frontend Runs card/reference を削除し、404 test を追加。
- Host/Worker APIs は DB authority ではなく live runtime views として維持。
- Legacy bootstrap tables は non-canonical `legacy_*` preservation tables に demote。
- Legacy `workspaces` は `legacy_workspaces` に preserve し、active canonical `workspaces` を v0 column set で作り直して既存行を copy。
- Post-upgrade `upsert_workspace()` for new workspace id が通る regression test を追加。
- `schema-v0.md` に SQLite version 2 / legacy preservation alignment notes を追加。

統合・検証:
- Merge commit: `38bd122d merge: workspace db schema v0`
- Implementation commits: `5149ab70`, `d89ace5b`
- Reviewer final verdict: approve
- Validation passed: `cargo fmt --check`, `git diff --check HEAD^1..HEAD`, `cargo test -p yoi-workspace-server`, `cargo check -p yoi-workspace-server`, `deno task check`, `deno task build`, `cargo run -p yoi -- ticket doctor`, and `nix build .#yoi --no-link`。

範囲外:
- Ticket/Objective write authority migration to DB is not implemented。
- Host/Worker canonical DB tables are not added。
- Validation/CI result tables and Actor table are not added。
- Full TicketEvents/TicketWorkerLinks/Artifacts write surfaces are not implemented beyond schema/bootstrap alignment。