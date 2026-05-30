---
id: 20260530-062852-refresh-stale-docs
slug: refresh-stale-docs
title: Docs: refresh stale architecture and operation docs
status: open
kind: task
priority: P2
labels: [docs, maintenance]
created_at: 2026-05-30T06:28:52Z
updated_at: 2026-05-30T06:39:11Z
assignee: null
legacy_ticket: null
---

## Background

Read-only docs audit Pod `docs-stale-audit-20260530` reported that several high-level docs still describe older protocol/session/tool/TUI behavior. The stale content is concentrated in architecture, compaction, TUI keybindings, and older plan documents.

The current authoritative sources are the code, `work-items/`, `KNOWN_ISSUES.md`, `AGENTS.md`, and git history. `docs/report/` remains historical observations, not current spec authority.

## Scope

Update or clearly demote stale docs so readers do not mistake old plans for current behavior:

- `docs/architecture.md`
  - Refresh Protocol Method/Event overview.
  - Refresh session persistence description for current `session_id` / `segment_id` / `SegmentOrigin` / Pod metadata split.
  - Refresh built-in tools description beyond only file tools.
  - Clarify the coarse `1 Pod = 1 process = 1 session` statement after Pod metadata / session / segment separation.
- `docs/compaction.md`
  - Refresh compact/session/segment descriptions to match current session-store and segment semantics.
- `docs/tui-keybindings.md`
  - Refresh `Ctrl-R` behavior and current command/queue/completion controls.
- `docs/plan/llm_presistence.md`
  - Mark as superseded/archival or replace with current-session persistence pointers.
- `docs/plan/ai-maintainer.md`
  - Replace old `TODO.md` / `tickets/` authority language with current `tickets.sh` + `work-items/` operation.

## Acceptance criteria

- High-priority stale claims from the docs audit are either corrected or explicitly marked historical/superseded.
- Updated docs consistently describe current Pod metadata vs session-store vs runtime registry responsibilities at a high level.
- Updated docs do not claim `TODO.md` / legacy `tickets/` are the active work-item authority.
- Updated docs do not claim `Ctrl-R` is removed; TUI keybindings reflect current behavior sufficiently for users.
- No generated report/history document is rewritten as if it were current spec authority.
- `./tickets.sh doctor` and `git diff --check` pass.
