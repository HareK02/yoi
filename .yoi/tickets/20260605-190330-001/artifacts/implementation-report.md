# Implementation report: ticket-role-pod-launcher

## Worktree / branch

- Worktree: `/home/hare/Projects/yoi/.worktree/ticket-role-pod-launcher`
- Branch: `work/ticket-role-pod-launcher`

## Commits

- `4bf0e27 feat: add ticket role pod launcher`
- `dd70517 fix: harden ticket role launch execution`

## Summary

Added a reusable Ticket role Pod launcher in the `client` crate so TUI/future CLI surfaces can build and execute fixed Ticket-role Pod launches without depending on `pod` internals or duplicating role/profile/workflow prompt construction.

The launcher uses `.yoi/ticket.config.toml` role configuration, preserves Profile ownership of durable system/role behavior, and sends concrete Ticket/action context as the first committed `Method::Run` input.

## Final module/API layout

- `crates/client/src/ticket_role.rs`
  - `TicketRef`
  - `TicketRoleLaunchContext`
  - `TicketRoleLaunchPlan`
  - `TicketRoleLaunchResult`
  - `TicketRoleLaunchError`
  - `plan_ticket_role_launch(...)`
  - `plan_ticket_role_launch_with_config(...)`
  - `launch_ticket_role_pod(...)`
- `crates/client/src/lib.rs`
  - re-exports the Ticket role launcher API.

## Behavior

Supported roles come from `ticket::config::TicketRole`:

- `intake`
- `orchestrator`
- `coder`
- `reviewer`
- `investigator`

The launcher:

- loads `.yoi/ticket.config.toml` with `TicketConfig`;
- reads role `profile`, `workflow`, and optional `launch_prompt`;
- creates a deterministic launch plan;
- generates first-run input as `Segment::WorkflowInvoke { slug }` plus `Segment::Text { content }`;
- exposes unresolved `launch_prompt` refs as launch-plan/text metadata rather than treating them as system instruction;
- can execute concrete top-level profile launches with `spawn_pod`, `PodClient`, and `Method::Run`.

`profile = "inherit"` remains valid in launch planning but is rejected for top-level client execution with a bounded `UnsupportedInheritProfile` error, because top-level `--profile inherit` does not have child SpawnPod inheritance semantics.

`launch_ticket_role_pod(...)` waits for first-run acceptance evidence after sending `Method::Run`:

- accepts matching `Event::UserMessage`;
- accepts `Event::InvokeStart { kind: UserSend }`;
- accepts `Event::TurnStart`;
- reports `Event::Error`, stream close, and timeout as errors.

## Changed files

- `Cargo.lock`
- `crates/client/Cargo.toml`
- `crates/client/src/lib.rs`
- `crates/client/src/spawn.rs`
- `crates/client/src/ticket_role.rs`
- `package.nix`

## Review status

External sibling review initially requested changes with two blockers:

1. Default `profile = "inherit"` was being passed through top-level `--profile inherit`.
2. `launch_ticket_role_pod` returned success after writing `Method::Run` without acceptance evidence.

Both blockers were fixed in `dd70517`, and re-review approved with no blockers.

Remaining non-blocker follow-ups:

- Add fake-socket/client execution tests for acceptance/rejection/close/timeout behavior.
- Add an aggregate prompt/list cap; current implementation bounds individual fields but not the number of validation/report list entries.
- Future TUI/CLI integration must surface `UnsupportedInheritProfile` clearly or require concrete role profiles until an inheritance-aware launch path exists.

## Validation

Coder-reported validation for the initial implementation passed:

- `cargo test -p client ticket`
- `cargo test -p ticket`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `git diff --check`
- `./tickets.sh doctor`
- `nix build .#yoi --no-link`

Coder-reported validation for blocker fixes passed:

- `cargo test -p client ticket`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `git diff --check`
- `./tickets.sh doctor`
- `nix build .#yoi --no-link`

## Ready for merge

Yes.

This clears the launcher prerequisite for `tui-ticket-role-actions`.
