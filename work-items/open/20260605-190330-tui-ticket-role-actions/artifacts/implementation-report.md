# Implementation report: tui-ticket-role-actions

## Worktree / branch

- Worktree: `/home/hare/Projects/yoi/.worktree/tui-ticket-role-actions`
- Branch: `work/tui-ticket-role-actions`

## Commits

- `e125ebb feat: add TUI ticket role commands`
- `d288fa5 fix: require TUI ticket intake context`

## Summary

Added explicit TUI `:ticket` commands that launch fixed Ticket-role Pods through the shared `client` Ticket role launcher.

TUI now parses role actions, builds a `TicketRoleLaunchContext`, and calls `client::launch_ticket_role_pod(...)`. TUI does not construct `SpawnConfig`, profile selector semantics, workflow invocation segments, or first-run prompt content directly.

## Command syntax

Implemented commands:

```text
:ticket intake <context...>
:ticket route <ticket-id-or-slug> [instruction...]
:ticket investigate <ticket-id-or-slug> [instruction...]
:ticket implement <ticket-id-or-slug> [instruction...]
:ticket review <ticket-id-or-slug> [instruction...]
```

Role mapping:

- `intake` -> `TicketRole::Intake`
- `route` -> `TicketRole::Orchestrator`
- `investigate` -> `TicketRole::Investigator`
- `implement` -> `TicketRole::Coder`
- `review` -> `TicketRole::Reviewer`

`intake` requires non-empty context. Non-intake actions require a Ticket id/slug and preserve remaining text as the instruction.

## Changed files

- `crates/client/src/ticket_role.rs`
- `crates/tui/src/app.rs`
- `crates/tui/src/command.rs`
- `crates/tui/src/single_pod.rs`

## TUI plumbing

- Added `CommandAction::TicketRole(...)` as a TUI-local command action.
- `CommandExecution` can now carry either a Pod protocol method or local command action.
- `App` stores a pending command action after command submission.
- `single_pod.rs` handles the pending Ticket role action asynchronously and calls the shared client launcher.
- `PodRuntimeCommand` is passed narrowly into the single-Pod run loop/command-action handler so the launcher can start the role Pod.

## Diagnostics

- Launch start/success/failure are surfaced through actionbar notices.
- `UnsupportedInheritProfile` has a TUI-specific message explaining that top-level TUI Ticket launches require concrete role profiles in `.yoi/ticket.config.toml` until an inheritance-aware launch path exists.
- Missing non-intake Ticket refs and missing intake context return command diagnostics.

## Review status

External sibling review initially requested one blocker fix:

- `:ticket intake` accepted missing/whitespace-only context.

The blocker was fixed in `d288fa5`, and re-review approved with no blockers.

Remaining non-blocker follow-ups:

- Start-progress actionbar notice may be overwritten by final success/failure before a redraw during slow launches.
- `:help ticket` could describe each role in more detail.
- Execution-path tests stop at context construction/error formatting; a future launcher seam could test success/failure actionbar plumbing without spawning real Pods.

## Validation

Coder-reported validation for the initial implementation passed:

- `cargo test -p tui ticket --lib`
- `cargo test -p client ticket`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `git diff --check`
- `./tickets.sh doctor`
- `nix build .#yoi --no-link`

Reviewer-rerun validation passed:

- `git diff --check`
- `cargo test -p tui ticket --lib`
- `cargo test -p client ticket`
- `cargo fmt --check`
- `cargo check --workspace --all-targets`
- `./tickets.sh doctor`
- `cargo test -p tui --lib`
- `cargo test -p client`
- `nix build .#yoi`

Coder-reported validation for blocker fix passed:

- `cargo test -p tui ticket --lib`
- `cargo fmt --check`
- `git diff --check`
- `./tickets.sh doctor`

Reviewer re-ran focused blocker validation:

- `cargo test -p tui ticket_intake --lib`
- `cargo test -p tui ticket --lib`

## Ready for merge

Yes.
