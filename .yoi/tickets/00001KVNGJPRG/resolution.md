Workspace web に Repository / Objective pages を追加し、Orchestrator worktree の `orchestration` branch に統合した。

主な成果:
- Read-only Repository backend APIs を追加:
  - `/api/repositories`
  - `/api/repositories/local`
  - `/api/repositories/local/log`
  - `/api/repositories/local/tickets`
- Current workspace root を local Repository として扱う bounded repository summary を追加。
- Git repository では bounded branch/head/root/dirty/remote/recent log summary を返す。
- Non-Git workspace では `git.status = unavailable` と bounded diagnostics に degrade。
- Git log summary は recent commit hash/subject/author/timestamp に限定し、diff/patch/file content/blame/config は読まない。
- Remote URL summary は URL-scheme userinfo を redact。
- Read-only Ticket Kanban を Ticket state ごとに grouping し、workspace-local Ticket fallback diagnostic を含めた。
- Objective list summaries を filesystem Objective records から追加。
- Static SPA に hash-navigation Repository / Objectives pages を追加。
- Sidebar Repository/Objectives links を新 pages に接続。
- Repository page に summary, Git summary/log, diagnostics, read-only Ticket Kanban を表示。
- Objective page に title/state/updated_at/summary と detail placeholder links を表示。
- Ticket / Objective canonical authority remains filesystem read-through records; mutation API / DB canonical migration は追加していない。

統合・検証:
- Merge commit: `7ee702b1 merge: repository objective pages`
- Implementation commit: `ceb1ee3b feat: add repository objective pages`
- Reviewer final verdict: approve
- Validation passed: `cargo fmt --check`, `git diff --check HEAD^1..HEAD`, `cargo test -p yoi-workspace-server`, `cargo check -p yoi-workspace-server`, `deno task check`, `deno task build`, `cargo run -p yoi -- ticket doctor`, and `nix build .#yoi --no-link`。

範囲外 / follow-up notes:
- Repository CRUD/API, Objective edit/detail mutation, full Git browser/diff/file views, drag/drop Kanban, and write APIs were not implemented。
- Reviewer noted possible follow-ups: keep `#/objectives` sidebar link visible even on objective empty/error states, and further tighten remote URL sanitization for query-param or SCP-like token forms if needed。