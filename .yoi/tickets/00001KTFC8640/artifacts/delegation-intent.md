# Delegation intent: remove obsolete TUI `:ticket` commands

## Classification

`implementation-ready` cleanup.

The workspace panel now owns the user-facing Ticket/Intake/Orchestrator UX. The old single-Pod TUI `:ticket ...` commands are no longer needed and should be removed rather than kept as a fallback.

## Intent

Remove the obsolete TUI command family:

```text
:ticket intake ...
:ticket route ...
:ticket investigate ...
:ticket implement ...
:ticket review ...
```

Users should use:

- `yoi panel` for workspace Ticket/Intake/Orchestrator UI;
- `yoi ticket ...` for direct Ticket CLI operations;
- Ticket tools/workflows where appropriate.

## Worktree / branch

- worktree: `/home/hare/Projects/yoi/.worktree/remove-tui-ticket-commands`
- branch: `work/remove-tui-ticket-commands`

This ticket may read tracked `.yoi/tickets` records/design artifacts. Do not read or edit `.yoi/memory/`.

## Requirements

- Remove `:ticket` command registration/parsing from TUI command registry.
- Remove TUI parser tests that expect `:ticket ...` success and replace with unknown-command / unsupported behavior coverage where appropriate.
- Remove `CommandAction::TicketRole` and pending local action plumbing if it is only used by `:ticket`.
- Remove single-Pod runtime handling that launches Ticket role Pods from `:ticket` commands.
- Remove now-unused imports such as `TicketRole`, `TicketRef`, or `launch_ticket_role_pod` from single-Pod command handling if applicable.
- Keep shared `client::ticket_role` launcher code because `yoi panel` still uses it for Orchestrator/Intake.
- Keep `yoi panel` behavior unchanged.
- Keep `yoi ticket ...` CLI unchanged.
- Keep Ticket tools/workflows unchanged.
- Update active docs/help that still present `:ticket ...` as a supported route.
- Do not mass-rewrite historical closed Ticket records/artifacts that mention `:ticket`.
- Do not reintroduce `--multi`.

## Current code map

- `crates/tui/src/command.rs`
  - command registration, `ticket_command(...)`, `CommandAction::TicketRole`, parser tests.
- `crates/tui/src/single_pod.rs`
  - pending local action handling and `handle_ticket_role_command(...)` path.
- `crates/tui/src/app.rs` / nearby files
  - check for pending local action storage if command action removal requires cleanup.
- `docs/development/work-items.md` and active docs
  - remove user-facing `:ticket ...` instructions or replace with `yoi panel` / `yoi ticket ...`.

## Validation

Run at least:

- `cargo test -p tui command` or targeted command parser tests;
- `cargo test -p tui workspace_panel`;
- `cargo test -p tui multi_pod`;
- `cargo test -p yoi panel`;
- `cargo test -p client ticket_role` if shared launcher imports/usage are touched;
- `cargo check --workspace --all-targets`;
- `cargo fmt --check`;
- `git diff --check`;
- `cargo build -p yoi`;
- `target/debug/yoi ticket doctor`.

Run `nix build .#yoi --no-link` if feasible.

Also check active references:

```bash
rg -n ":ticket|ticket intake|ticket route|ticket implement|ticket review" docs crates/tui/src .yoi/workflow AGENTS.md README.md
```

Remaining matches should be code comments/tests intentionally covering unsupported behavior or historical closed records outside the active-reference search.

## Completion report

Report:

- worktree path / branch;
- commit hash;
- removed command/runtime paths;
- docs/tests updated;
- confirmation that panel/CLI/shared launcher remain intact;
- validation results;
- remaining historical references, if any.
