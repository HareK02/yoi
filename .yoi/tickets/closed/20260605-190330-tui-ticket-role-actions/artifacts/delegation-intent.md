# Delegation intent: TUI Ticket role actions

## Intent

Add explicit TUI commands/actions that launch fixed Ticket-role Pods through the shared `client` Ticket role launcher.

TUI should not duplicate role/profile/workflow/prompt construction. It should parse user commands, build a typed launcher context, call the client launcher, and surface success/failure diagnostics in the TUI.

## Worktree / branch

- worktree: `/home/hare/Projects/yoi/.worktree/tui-ticket-role-actions`
- branch: `work/tui-ticket-role-actions`

## Dependencies

`ticket-role-pod-launcher` is complete and merged. Use the client API added there:

- `client::TicketRoleLaunchContext`
- `client::TicketRef`
- `client::launch_ticket_role_pod`
- related launch errors/result types

## Requirements

- Add user-triggered TUI commands for fixed Ticket roles.
- Prefer a single `:ticket <action> ...` command if it fits the current command parser.
- Support at least these actions:
  - `:ticket intake <context...>`
  - `:ticket route <ticket-id-or-slug> [instruction...]`
  - `:ticket investigate <ticket-id-or-slug> [instruction...]`
  - `:ticket implement <ticket-id-or-slug> [instruction...]`
  - `:ticket review <ticket-id-or-slug> [instruction...]`
- Map actions to roles:
  - intake -> `TicketRole::Intake`
  - route -> `TicketRole::Orchestrator`
  - investigate -> `TicketRole::Investigator`
  - implement -> `TicketRole::Coder`
  - review -> `TicketRole::Reviewer`
- Use the shared launcher; do not build `SpawnConfig`, Profile selectors, Workflow segments, or first-run prompt content directly in TUI.
- Make command parsing/test behavior deterministic.
- Surface launcher failures as TUI command/actionbar diagnostics without crashing.
- Make `UnsupportedInheritProfile` especially clear: the user must configure a concrete role profile in `.yoi/ticket.config.toml` for top-level TUI launches.
- Keep actions explicit and user-triggered; no scheduler/automation.
- Do not add spawned-Pod panel or dashboard redesign.
- Do not add arbitrary role registry UI.

## Current code map

- `crates/tui/src/command.rs`
  - Current command registry is synchronous and returns `CommandExecution { method: Option<Method>, diagnostics, ... }`.
  - Existing commands are `help`, `noop`, `compact`, `rewind`, and `peer`.
  - Add a command action variant or equivalent so command parsing can request a Ticket role launch without overloading `protocol::Method`.
- `crates/tui/src/app.rs`
  - `submit_command` / `apply_command_execution` currently return `Option<Method>`.
  - May need a small result enum to carry either a Pod `Method` or a TUI-local action.
- `crates/tui/src/single_pod.rs`
  - Has `PodRuntimeCommand` at launch entrypoints, but `run_loop` currently only receives `App` + `PodClient`.
  - To execute role launch commands, pass `PodRuntimeCommand` into the loop/handler where needed.
  - Command execution can call `client::launch_ticket_role_pod(...)` asynchronously and then show success/error notice.
- `crates/client/src/ticket_role.rs`
  - Shared launcher API; use this instead of duplicating launch construction.

## Command semantics

MVP command syntax:

```text
:ticket intake <context...>
:ticket route <ticket-id-or-slug> [instruction...]
:ticket investigate <ticket-id-or-slug> [instruction...]
:ticket implement <ticket-id-or-slug> [instruction...]
:ticket review <ticket-id-or-slug> [instruction...]
```

For non-intake actions, treat the first argument as a Ticket id/slug. It is acceptable to use `TicketRef::slug(value)` in the MVP unless a clear id/slug parser already exists.

For `intake`, allow freeform context and no Ticket id.

Pod name may use the launcher default unless command syntax for `--name` is easy; do not overbuild CLI parsing.

## Non-goals

- Implementing the role launcher.
- Prompt resource resolution.
- Stateful workflow engine.
- Worktree creation automation.
- Multi-Pod dashboard redesign.
- Spawned-Pod panel.
- Scheduler/lease/queue automation.
- Generic role registry/UI.
- Arbitrary filesystem Ticket edits.

## Validation

Run at least:

- focused command parsing/action tests;
- `cargo test -p tui ticket --lib` or relevant focused TUI tests;
- `cargo test -p client ticket`;
- `cargo check --workspace --all-targets`;
- `cargo fmt --check`;
- `git diff --check`;
- `./tickets.sh doctor`.

Run `nix build .#yoi --no-link` if feasible.

## Completion report

Report:

- worktree path / branch;
- commit hash;
- command syntax implemented;
- how command execution calls the shared launcher;
- how diagnostics are surfaced;
- validation results;
- unresolved risks/follow-ups;
- whether ready for external review.
