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
