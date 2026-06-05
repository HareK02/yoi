<!-- event: create author: tickets.sh at: 2026-06-05T19:03:30Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-06-05T19:06:17Z -->

## Plan

Plan: implement after `ticket-role-pod-launcher` lands.

TUI should expose explicit user-triggered Ticket role actions/commands and call the shared launcher. It should not duplicate profile/config/prompt/workflow launch construction, introduce automatic scheduling, or add a spawned-Pod panel in this ticket.


---

<!-- event: plan author: hare at: 2026-06-05T19:36:50Z -->

## Plan

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


---

<!-- event: review author: hare at: 2026-06-05T20:07:21Z status: approve -->

## Review: approve

# External review: tui-ticket-role-actions

## 1. Result: approve after blocker fix

Initial review requested changes because `:ticket intake` accepted missing context. Re-review of commit `d288fa590188bb700257e3cfa386b168661d9613` confirms that blocker is resolved and no new blocker was introduced.

## 2. Summary of implementation

The commit adds a new `:ticket <action> ...` TUI command in `crates/tui/src/command.rs`, carries successful parses as a TUI-local `CommandAction::TicketRole`, stores the pending local action on `App`, and handles it from the single-Pod event loop in `crates/tui/src/single_pod.rs`.

Execution builds a `TicketRoleLaunchContext` with the current working directory, fixed `TicketRole`, optional `TicketRef::slug(...)`, and optional freeform instruction, then calls `client::launch_ticket_role_pod(...)`. The TUI does not construct `SpawnConfig`, profile selectors, workflow invocation segments, or prompt contents directly. `crates/client/src/ticket_role.rs` only reexports `TicketRole` for this TUI-facing use.

## 3. Requirement-by-requirement assessment

- **Command syntax**: Met after `d288fa590188bb700257e3cfa386b168661d9613`. Implemented nested syntax close to the requested surface:
  - `:ticket intake <context...>` parses when context is present, and `:ticket intake` / whitespace-only context now reject with `Invalid arguments. Usage: ticket intake <context...>`.
  - `:ticket route <ticket-id-or-slug> [instruction...]`, `investigate`, `implement`, and `review` require a first ticket reference and preserve remaining text as instruction.
- **Role mapping**: Met. Mappings are `intake -> Intake`, `route -> Orchestrator`, `investigate -> Investigator`, `implement -> Coder`, `review -> Reviewer`.
- **Shared launcher use**: Met. `single_pod.rs` builds only `TicketRoleLaunchContext` and calls `launch_ticket_role_pod(...)`; no TUI-side `SpawnConfig`, profile semantics, workflow segments, or prompt construction were introduced.
- **Command action plumbing locality**: Met. The new `CommandAction` path is local, and existing `compact`, `rewind`, `peer`, help, and completion behavior remains structurally unchanged. Full `cargo test -p tui --lib` also passed.
- **`PodRuntimeCommand` threading**: Met. The value is passed narrowly into the single-Pod run loop and command-action handler so the shared launcher can spawn the role Pod.
- **Diagnostics**: Met for the reviewed command requirements. Success and failure are surfaced through actionbar notices, `UnsupportedInheritProfile` gets a clear TUI-specific message, missing non-intake ticket references are diagnosed, and missing intake context is now diagnosed.
- **Unsupported inherit profile explanation**: Met. The special-case message clearly tells the user to configure concrete role profiles in `.yoi/ticket.config.toml` for top-level TUI ticket launches.
- **Non-goals / scope control**: Met. I did not find scheduler/automation, spawned-Pod panel, dashboard redesign, generic role UI, prompt resolution, worktree automation, or arbitrary Ticket filesystem edits.
- **Tests**: Met for the blocker fix. Parsing and context construction tests exist, an inherit-profile diagnostic formatting test exists, and focused parser coverage now rejects missing/whitespace-only intake context. The launcher execution path is still covered indirectly by context construction rather than by a mocked launcher call/actionbar transition.
- **Validation**: Met for the commands I reran; see section 6.

## 4. Blockers

No blockers remain after `d288fa590188bb700257e3cfa386b168661d9613`.

- Resolved: `:ticket intake` previously accepted an empty context; it now rejects missing or whitespace-only context with `Invalid arguments. Usage: ticket intake <context...>`, and focused parser coverage was added.

## 5. Non-blockers / follow-ups

- The `Launching ticket ... Pod...` actionbar notice is set immediately before awaiting `launch_ticket_role_pod(...)` (`crates/tui/src/single_pod.rs:563-589`). Because the event loop does not redraw between setting that notice and awaiting the launch, users may only see the final success/failure notice for slow launches. This is acceptable for this MVP, but a follow-up could force a draw or move launch work to an async task if start-progress visibility matters.
- Help text for `:ticket` names the fixed actions but does not explain each role individually. The behavior is discoverable enough for MVP, but richer `:help ticket` text would better satisfy the acceptance criterion that command/action help makes clear what each role does.
- Execution-path tests stop at context construction and error formatting. A future small seam around the launcher call would allow direct tests for actionbar success/failure plumbing without spawning real Pods.

## 6. Validation assessed or rerun

Reran from `/home/hare/Projects/yoi/.worktree/tui-ticket-role-actions`:

- `git diff --check` — passed.
- `cargo test -p tui ticket --lib` — passed, 5 tests.
- `cargo test -p client ticket` — passed, 6 tests.
- `cargo fmt --check` — passed.
- `cargo check --workspace --all-targets` — passed.
- `./tickets.sh doctor` — passed (`doctor: ok`).
- `cargo test -p tui --lib` — passed, 224 tests.
- `cargo test -p client` — passed, 11 tests plus doc-tests.
- `nix build .#yoi` — passed; Nix emitted a dirty-tree warning for the worktree. The transient `result` symlink created by Nix was removed afterward; `git status --short` showed no tracked changes.

## 7. Residual risk

After the intake-context diagnostic is fixed, residual risk is low. The implementation stays within the intended TUI-command surface and delegates launch semantics to the client launcher. The remaining risks are mostly UX/test-depth issues around live launch progress visibility and lack of a direct mocked launcher execution test.

## 8. Re-review of blocker fix (`d288fa590188bb700257e3cfa386b168661d9613`)

### Result: approve

The blocker is resolved. The fix updates `ticket_args` so `intake` now requires at least one non-whitespace argument and returns `Invalid arguments. Usage: ticket intake <context...>` when context is missing. The existing `ticket_command` path then continues to pass the non-empty intake context as `instruction`, with no Ticket reference, preserving the intended Intake launch behavior.

Focused parser coverage was added for both `:ticket intake` and whitespace-only `:ticket intake    ` rejection. The change is limited to `crates/tui/src/command.rs`; I did not find any new blocker or source-scope expansion in this fix.

Validation rerun from `/home/hare/Projects/yoi/.worktree/tui-ticket-role-actions`:

- `cargo test -p tui ticket_intake --lib` — passed.
- `cargo test -p tui ticket --lib` — passed.

No additional full-build validation was rerun for this narrow parser-only fix.


---

<!-- event: implementation_report author: hare at: 2026-06-05T20:07:22Z -->

## Implementation report

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


---
