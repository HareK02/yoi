TUI Ticket role actions are complete and merged.

Implementation:

- `e125ebb feat: add TUI ticket role commands`
- `d288fa5 fix: require TUI ticket intake context`
- merge commit: `5d3209d merge: add tui ticket role actions`

Summary:

- Added explicit TUI `:ticket` commands for fixed Ticket roles:
  - `:ticket intake <context...>`
  - `:ticket route <ticket-id-or-slug> [instruction...]`
  - `:ticket investigate <ticket-id-or-slug> [instruction...]`
  - `:ticket implement <ticket-id-or-slug> [instruction...]`
  - `:ticket review <ticket-id-or-slug> [instruction...]`
- Mapped actions to fixed Ticket roles:
  - intake -> Intake
  - route -> Orchestrator
  - investigate -> Investigator
  - implement -> Coder
  - review -> Reviewer
- Added TUI-local `CommandAction::TicketRole(...)` plumbing.
- TUI builds `TicketRoleLaunchContext` and calls the shared `client::launch_ticket_role_pod(...)` launcher.
- TUI does not construct `SpawnConfig`, profile selector semantics, workflow segments, or first-run prompt content directly.
- `PodRuntimeCommand` is passed narrowly into the single-Pod command handling path for launching role Pods.
- Success/failure is surfaced through actionbar notices.
- `UnsupportedInheritProfile` receives a clear message explaining that top-level TUI Ticket launches require concrete role profiles in `.yoi/ticket.config.toml` until an inheritance-aware launch path exists.
- No scheduler, spawned-Pod panel, dashboard redesign, generic role UI, prompt resolution, worktree automation, or arbitrary Ticket filesystem edits were introduced.

Review:

- External sibling review initially requested one blocker fix: `:ticket intake` accepted missing/whitespace-only context.
- Coder fixed it in `d288fa5`; `:ticket intake` now requires non-empty context and has focused parser tests.
- Re-review approved with no blockers.

Non-blocker follow-ups:

- Launch-start actionbar notice may not be visible before final success/failure during slow launches because the loop awaits launch before redraw.
- `:help ticket` could explain each role in more detail.
- A future launcher seam could test actionbar success/failure plumbing without spawning real Pods.

Post-merge validation passed:

- `cargo test -p tui ticket --lib`
- `cargo test -p client ticket`
- `cargo test -p tui --lib`
- `cargo test -p client`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `git diff --check`
- `./tickets.sh doctor`
- `nix build .#yoi --no-link`
