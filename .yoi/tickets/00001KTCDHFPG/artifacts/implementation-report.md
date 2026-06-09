# Implementation report: ticket-config-role-profile-mapping

## Worktree / branch

- Worktree: `/home/hare/Projects/yoi/.worktree/ticket-config-role-profile-mapping`
- Branch: `work/ticket-config-role-profile-mapping`

## Commits

- `767870a ticket: add workspace ticket config`
- `8fab67b ticket: reject nix profile selectors`

## Summary

Implemented `.yoi/ticket.config.toml` as workspace-local Ticket orchestration configuration with fixed Ticket role slots and wired the configured backend root into the existing Ticket built-in feature adapter.

The implementation keeps Ticket role configuration narrow:

- fixed roles only: `intake`, `orchestrator`, `coder`, `reviewer`, `investigator`;
- role fields: `profile`, optional `launch_prompt`, optional `workflow`;
- no `system_instruction` role field;
- durable role/system behavior remains owned by the selected Profile.

## Final module/API layout

Added `crates/ticket/src/config.rs`, exported as `ticket::config`.

Main public API:

- `TicketConfig`
  - `load_workspace(workspace_root)`
  - `default_for_workspace(workspace_root)`
  - `backend_root()`
  - `role(role)`
  - `profile_for(role)`
  - `launch_prompt_for(role)`
  - `workflow_for(role)`
- `TicketBackendConfig`
- `TicketBackendKind`
- `TicketRole`
- `ProfileSelectorRef`
- `PromptRef`
- `WorkflowRef`
- `TicketConfigError`
- `TICKET_CONFIG_RELATIVE_PATH = ".yoi/ticket.config.toml"`

The `ticket` crate keeps lightweight string refs and does not depend on `pod` or `manifest`.

## Schema/defaults implemented

Config path:

```toml
.yoi/ticket.config.toml
```

Backend:

```toml
[backend]
kind = "local"
root = "work-items"
```

Role example:

```toml
[roles.coder]
profile = "project:coder"
launch_prompt = "$workspace/prompts/ticket-coder"
workflow = "multi-agent-workflow"
```

Defaults when the config file is missing:

- backend: local `<workspace>/work-items`;
- all role profiles: `inherit`;
- launch prompts: none;
- workflows:
  - intake: `ticket-intake-workflow`;
  - orchestrator: `ticket-orchestrator-routing`;
  - coder: `multi-agent-workflow`;
  - reviewer: `multi-agent-workflow`;
  - investigator: `ticket-orchestrator-routing`.

Validation rejects unknown top-level fields, unknown backend fields, unknown role fields, unknown roles, unsupported backend kinds, malformed/empty refs, path-like profile selector values, `.lua`, and `.nix` profile selector values.

## Pod Ticket feature adapter wiring

Updated `crates/pod/src/feature/builtin/ticket.rs` so `TicketFeature::for_workspace(...)` loads `ticket::config::TicketConfig`.

Behavior:

- missing config uses documented defaults, preserving previous `<workspace>/work-items` behavior;
- valid config uses configured `[backend].root`;
- malformed config fails closed: Ticket tools are not registered and a feature diagnostic is emitted;
- missing/unusable backend root preserves existing no-register behavior;
- tool authority continues to use `HostAuthority::TicketBackend { root }` for the configured backend root.

## Changed files

- `Cargo.lock`
- `crates/pod/src/feature/builtin/ticket.rs`
- `crates/ticket/Cargo.toml`
- `crates/ticket/src/config.rs`
- `crates/ticket/src/lib.rs`
- `package.nix`

## Review status

External sibling review initially requested one blocker fix:

- `ProfileSelectorRef` accepted `*.nix` profile selectors while existing `SpawnPod.profile` validation rejects them.

The blocker was fixed by commit `8fab67b` and re-review approved with no blockers.

Remaining non-blocker follow-ups:

- `HostAuthority::TicketBackend { root }` is derived from the configured path while the actual backend uses a canonicalized usable root; future explicit grant/audit comparisons should normalize consistently.
- Pod adapter root usage could be strengthened with an execution-level tool test against the configured root.

## Validation

Coder-reported validation for the main implementation passed:

- `cargo test -p ticket`
- `cargo test -p pod ticket --lib`
- `cargo test -p pod feature --lib`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `git diff --check`
- `./tickets.sh doctor`
- `nix build .#yoi --no-link`

Coder-reported validation for the blocker fix passed:

- `cargo test -p ticket config`
- `cargo test -p ticket`
- `cargo fmt --check`
- `git diff --check`

## Ready for merge

Yes.
