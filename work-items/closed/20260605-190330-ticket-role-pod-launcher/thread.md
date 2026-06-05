<!-- event: create author: tickets.sh at: 2026-06-05T19:03:30Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-06-05T19:06:16Z -->

## Plan

Preflight result: `implementation-ready`.

Implement `ticket-role-pod-launcher` before TUI actions so TUI does not own role/profile/prompt/workflow launch construction.

Design decisions:

- Fixed Ticket roles only: intake, orchestrator, coder, reviewer, investigator.
- Role profile/workflow/launch_prompt refs come from `.yoi/ticket.config.toml`.
- Profile owns durable role/system behavior.
- Launcher generates the first committed user/task message for a concrete Ticket/action.
- Dynamic content must be sent through `Method::Run`, not hidden context injection.
- Prefer a client-level API so TUI can use it without depending on `pod` internals.
- Configured `launch_prompt` refs may be exposed but not resolved if no suitable prompt-resource API exists below `pod`; do not treat them as system instruction.

Follow-up after this lands: implement `tui-ticket-role-actions` using the launcher.


---

<!-- event: plan author: hare at: 2026-06-05T19:07:09Z -->

## Plan

# Delegation intent: Ticket role Pod launcher

## Intent

Implement a reusable Ticket role Pod launcher so TUI and later CLI/orchestrator surfaces can launch fixed Ticket-role Pods without duplicating profile/config/workflow/prompt construction logic.

The launcher should use `.yoi/ticket.config.toml` fixed role configuration, generate first-run task content, and keep dynamic Ticket/action context in `Method::Run` input rather than hidden context injection.

## Worktree / branch

- worktree: `/home/hare/Projects/yoi/.worktree/ticket-role-pod-launcher`
- branch: `work/ticket-role-pod-launcher`

## Requirements

- Support fixed Ticket roles only:
  - `intake`
  - `orchestrator`
  - `coder`
  - `reviewer`
  - `investigator`
- Load `.yoi/ticket.config.toml` through `ticket::config::TicketConfig`.
- Use role `profile` selector as the child Pod profile selector.
- Use role `workflow` ref as model-visible workflow input in the first run.
- Generate first committed user/task message content from a typed launch context.
- Keep selected Profile responsible for durable system/role behavior; do not add `system_instruction` support.
- Do not inject dynamic instructions into context outside history; first-run prompt/task content must go through `Method::Run`.
- Prefer a client-level API so TUI can use it without depending on `pod` crate internals.
- Avoid duplicating current runtime spawn internals if existing `client::spawn_pod`, `PodClient`, and `protocol::Method::Run` can be used cleanly.
- Expose a launch planning API even if full execution is constrained, so TUI work has a stable boundary.

## Suggested module placement

Preferred:

- `crates/client/src/ticket_role.rs`
- exports from `crates/client/src/lib.rs`

Rationale:

- `tui` already depends on `client`.
- `client` can depend on `ticket` without introducing `tui -> pod`.
- `client` owns host-side spawn/socket mechanics.

If current crate boundaries make full execution awkward, implement the pure planning API in `client` first and clearly report the execution gap.

## Suggested API shape

Exact names can change, but keep the surface typed:

```rust
pub enum TicketRoleLaunchKind {
    Intake,
    Orchestrator,
    Coder,
    Reviewer,
    Investigator,
}

pub struct TicketRoleLaunchContext {
    pub workspace_root: PathBuf,
    pub role: TicketRole,
    pub pod_name: Option<String>,
    pub ticket: Option<TicketRefLike>,
    pub user_instruction: Option<String>,
    pub intent_packet: Option<String>,
    pub worktree_path: Option<PathBuf>,
    pub branch: Option<String>,
    pub validation: Vec<String>,
    pub report_expectations: Vec<String>,
}

pub struct TicketRoleLaunchPlan {
    pub role: TicketRole,
    pub pod_name: String,
    pub profile: String,
    pub workflow: String,
    pub launch_prompt_ref: Option<String>,
    pub run_segments: Vec<protocol::Segment>,
}
```

Use existing `ticket::config::TicketRole` if practical rather than duplicating role enum. Avoid exposing pod internals.

## Prompt generation expectations

Generated first-run text should include:

- role name;
- Ticket id/slug if present;
- user/action instruction;
- workflow slug;
- launch_prompt ref if configured but unresolved;
- intent packet if provided;
- worktree path / branch if provided;
- validation/report expectations if provided;
- reminder that Profile supplies system/role behavior and the Workflow supplies process.

Prefer typed `Segment::WorkflowInvoke` plus text if current protocol/client path supports it. If not, include workflow slug in text and document the limitation.

## Non-goals

- TUI command/action UI.
- Stateful workflow engine.
- Phase-specific prompts/tool gating.
- Role-level `system_instruction` support.
- Prompt resource resolution if it requires moving prompt loader APIs across crates.
- Changing Profile resolution semantics.
- Changing `SpawnPod` tool semantics in the `pod` crate.
- Scheduler/lease/queue automation.
- Worktree creation automation.

## Validation

Run at least:

- `cargo test -p client ticket` or focused client tests;
- `cargo test -p ticket` if touched;
- `cargo check --workspace --all-targets`;
- `cargo fmt --check`;
- `git diff --check`;
- `./tickets.sh doctor`.

Run `nix build .#yoi --no-link` if feasible.

## Completion report

Report:

- worktree path / branch;
- commit hash;
- final module/API layout;
- whether launch execution is implemented or only planning;
- generated prompt / workflow segment behavior;
- how role profile config is used;
- validation results;
- unresolved risks/follow-ups;
- whether `tui-ticket-role-actions` can proceed.


---

<!-- event: review author: hare at: 2026-06-05T19:32:52Z status: approve -->

## Review: approve

# External review: Ticket role Pod launcher

## 1. Result

request changes

## 2. Summary of implementation

The implementation adds `crates/client/src/ticket_role.rs` and re-exports a client-level Ticket role launch API. The new module builds a `TicketRoleLaunchPlan` from `ticket::config::TicketConfig`, fixed `TicketRole` values, role profile/workflow/launch_prompt refs, a generated first-run prompt, and `Segment::WorkflowInvoke` + `Segment::Text` run input. It also adds `launch_ticket_role_pod`, which calls `client::spawn_pod`, connects with `PodClient`, and writes `Method::Run`.

The implementation is small and mostly stays on the intended boundary: no TUI UI, no scheduler/queue, no workflow engine, no worktree automation, no `pod` SpawnPod-tool changes, and no role-level system-instruction support were introduced.

## 3. Requirement-by-requirement assessment

- Appropriate crate / TUI boundary: mostly satisfied. The launcher lives in `client`, and the diff does not make TUI depend on `pod`.
- Fixed Ticket roles only: satisfied. It uses `ticket::config::TicketRole` rather than adding an arbitrary registry.
- `.yoi/ticket.config.toml` loading: satisfied for planning. `plan_ticket_role_launch` calls `TicketConfig::load_workspace`.
- Role profile selector as child profile selector: not satisfied for execution. The plan preserves the string, but execution passes `inherit` through top-level `--profile`, where it is not the SpawnPod child-profile special selector.
- Profile semantics unchanged: not satisfied for execution. `inherit` only has child/inherited-manifest semantics in `pod::spawn::tool`; top-level profile parsing treats it as a named registry profile.
- No role-level `system_instruction`: satisfied. Unknown `system_instruction` remains rejected by config parsing, and the launcher does not add overlay support.
- Dynamic content through `Method::Run`: satisfied in planning and mostly in execution shape. The launch content is represented as `Method::Run` segments, not hidden context injection.
- First-run input uses `Segment::WorkflowInvoke` plus `Segment::Text`: satisfied.
- `launch_prompt` refs unresolved/exposed: satisfied. The plan exposes `launch_prompt_ref` and the generated text labels it as unresolved.
- Prompt text bounded/deterministic/useful: partially satisfied. Individual fields are trimmed and capped, and the included context is useful. The total prompt is not globally bounded because validation/report vectors are unbounded.
- Actual launch execution safe/consistent: not satisfied. The function returns after writing to the socket, without waiting for run acceptance/commit evidence or surfacing rejection events.
- Error handling/diagnostics: partially satisfied. Config/spawn/connect/write errors are typed, but run rejection/AlreadyRunning/commit failure cannot be reported by `launch_ticket_role_pod` because it never reads acknowledgement events.
- Dependencies: acceptable. `ticket` and `thiserror` are justified; `tempfile` is test-only; `package.nix` hash was updated. No suspicious dependency expansion was observed.
- Non-goals: satisfied. I did not see TUI UI, scheduler/lease/queue, workflow engine, worktree automation, `pod` SpawnPod-tool changes, or broad refactors.
- Tests: mostly satisfied for planning requirements. Tests cover default config planning, configured refs, prompt content for intake/orchestrator/reviewer, caller-provided Pod name, malformed config, and no `system_instruction`. They do not cover execution acknowledgement/failure behavior.

## 4. Blockers

### Blocker 1: Default `inherit` role profile cannot be executed correctly through `client::spawn_pod`

`TicketConfig` defaults every role to `profile = "inherit"` (`crates/ticket/src/config.rs:214-217`). The launcher preserves that value and always converts it into `SpawnConfig { profile: Some(self.profile.clone()), ... }` (`crates/client/src/ticket_role.rs:122-127`). `client::spawn_pod` then renders this as top-level CLI args `--profile inherit --profile-pod-name ...` (`crates/client/src/spawn.rs:132-137`).

That does not invoke the child-profile `inherit` semantics. Top-level profile parsing only treats `default` specially; `inherit` falls through to `ProfileSelector::Named { name: "inherit" }` (`crates/manifest/src/profile.rs:93-108`, via `crates/pod/src/entrypoint.rs:106-108`). The special `inherit` behavior exists in `pod::spawn::tool`'s SpawnPod-profile parser, not in the client top-level spawn path.

As a result, `launch_ticket_role_pod` with the default Ticket role config is expected to fail profile resolution unless a registry profile named `inherit` happens to exist, and if such a profile exists it would use different semantics. This breaks the MVP execution path and violates the requirement not to change Profile semantics.

A fix should make this boundary explicit. Either implement execution through a path that really supports child-profile `inherit`, or make execution fail closed / remain planning-only for `inherit` with a bounded diagnostic until a correct inheritance source is available. Do not reinterpret `inherit` as `default` in the client launcher.

### Blocker 2: `launch_ticket_role_pod` does not confirm that the first `Method::Run` was accepted/committed

`launch_ticket_role_pod` spawns the Pod, connects, writes `Method::Run`, and returns success immediately after `PodClient::send` succeeds (`crates/client/src/ticket_role.rs:214-226`). `PodClient::send` only writes the JSON line (`crates/client/src/pod_client.rs:34-35`); it does not wait for `Event::UserMessage`, `Event::InvokeStart { kind: UserSend }`, `Event::TurnStart`, or `Event::Error`.

The ticket requires a first committed user/task message, and the review objective asks whether actual execution is safe/consistent with existing client behavior. Existing one-shot spawn delivery in `pod::spawn::comm_tools::send_run_and_confirm` explicitly drains initial socket events and waits for acceptance or rejection evidence (`crates/pod/src/spawn/comm_tools.rs:337-408`). The new launcher lacks equivalent confirmation and therefore can report a successful launch even when the run is rejected after the write, the Pod is already running, or the connection closes before acceptance evidence.

A fix should either add a client-level `send_run_and_confirm`-style path that supports typed `Vec<Segment>`, with bounded timeouts and useful rejection diagnostics, or downgrade the execution API so it does not claim the first run was launched/committed.

## 5. Non-blockers / follow-ups

- The generated prompt caps individual string fields at 8,000 chars, but `validation` and `report_expectations` list lengths are unbounded. Consider an aggregate prompt cap or per-list item-count cap before wiring this to UI surfaces.
- The public client re-exports do not re-export `ticket::config::TicketRole`; TUI can still add/use `ticket`, but a client-side re-export may keep the launcher API easier to consume.
- Execution-path tests should be added with a fake socket once Blocker 2 is addressed, especially for acceptance, rejection, and already-running diagnostics.

## 6. Validation assessed or rerun

Read/inspected:

- Ticket item and delegation intent.
- `crates/client/src/ticket_role.rs`, `spawn.rs`, `pod_client.rs`.
- `crates/protocol/src/lib.rs` for `Segment::WorkflowInvoke` support.
- `crates/ticket/src/config.rs` for fixed roles/default profile behavior.
- Relevant profile/spawn parsing paths in `manifest` and `pod`.
- Diff against `develop`.

Commands run, all from `/home/hare/Projects/yoi/.worktree/ticket-role-pod-launcher`:

- `git status --short`
- `git diff --stat develop...HEAD`
- `git diff --name-status develop...HEAD`
- `git diff --check develop...HEAD` — no whitespace diagnostics observed.
- `git rev-parse HEAD`
- `git merge-base develop HEAD`
- `git diff --name-only develop...HEAD`

I did not rerun `cargo test`, `cargo check`, `cargo fmt`, `tickets.sh doctor`, or `nix build`, because this external review was constrained to focused read-only validation commands and those commands would write build/check artifacts.

## 7. Residual risk

After the blockers are fixed, the main residual risk is deciding the correct ownership of role Pod execution semantics: a client/TUI launcher can plan the request cleanly, but `inherit` and confirmed first-run delivery are child-Pod semantics that need a deliberate bridge rather than accidental top-level profile spawning. Once that bridge is explicit and tested, the rest of the implementation looks like a suitable foundation for TUI/future CLI role actions.

---

# Re-review: blocker fixes in `dd70517f967424887daf3f30e5aed5b1e6f459c8`

## 1. Result

approve

## 2. Summary of fix

The follow-up commit hardens the execution path without changing the planning model. `TicketRoleLaunchPlan::spawn_config(...)` now returns `Result<SpawnConfig, TicketRoleLaunchError>` and rejects `profile == "inherit"` with a clear fail-closed diagnostic before top-level `client::spawn_pod` can reinterpret it as a normal `--profile inherit` selector. `launch_ticket_role_pod(...)` now sends the first `Method::Run` and then waits, with a 10 second timeout, for run acceptance evidence from the Pod event stream.

## 3. Blocker reassessment

### Previous Blocker 1: default `inherit` profile executed through top-level `--profile`

Resolved.

Planning still preserves the configured/default profile string, including `inherit`, so pure launch planning remains usable. Execution now calls `plan.spawn_config(runtime_command)?`, and `spawn_config` returns `TicketRoleLaunchError::UnsupportedInheritProfile` when `self.profile == "inherit"` before constructing `SpawnConfig`. The error message is bounded and explicit: `Ticket role profile 'inherit' cannot be used for top-level launch execution; configure a concrete role profile selector`.

This satisfies the requested fix and avoids changing Profile semantics. It leaves default-role execution unavailable until a concrete role profile is configured or a correct inheritance-capable launch path exists, which is preferable to accidental top-level reinterpretation.

### Previous Blocker 2: first `Method::Run` write was not confirmed

Resolved for the requested acceptance boundary.

After `PodClient::send(&plan.run_method())`, `launch_ticket_role_pod` now calls `wait_for_run_acceptance`. That helper waits for:

- `Event::UserMessage { segments }` matching the sent segments;
- `Event::InvokeStart { kind: InvokeKind::UserSend }`;
- `Event::TurnStart { .. }`.

It treats `Event::Error { code, message }` as `RunRejected`, stream close as `RunAcceptanceClosed`, and timeout as `RunAcceptanceTimeout`. This is bounded and provides useful TUI-facing diagnostics.

## 4. New blockers

None found in the focused re-review.

## 5. Non-blockers / follow-ups

- Execution acknowledgement behavior is not covered by new fake-socket/client tests. I do not consider that a merge blocker for this fix because the implementation is small and directly follows the protocol events, but adding acceptance/rejection/close/timeout tests would reduce regression risk.
- The earlier non-blocker about aggregate prompt/list bounds remains: individual fields are capped, but list lengths are not globally bounded.

## 6. Validation assessed or rerun

Read/inspected:

- `crates/client/src/ticket_role.rs`
- `crates/client/src/pod_client.rs`
- `crates/client/src/spawn.rs`
- `crates/protocol/src/lib.rs`
- diff from `4bf0e2715c50bfe3adb0e42385fbe86470e3eb3d` to `dd70517f967424887daf3f30e5aed5b1e6f459c8`

Commands run, all from `/home/hare/Projects/yoi/.worktree/ticket-role-pod-launcher`:

- `git rev-parse HEAD` — confirmed `dd70517f967424887daf3f30e5aed5b1e6f459c8`
- `git status --short` — no source changes reported
- `git diff --stat 4bf0e2715c50bfe3adb0e42385fbe86470e3eb3d..dd70517f967424887daf3f30e5aed5b1e6f459c8`
- `git diff --check 4bf0e2715c50bfe3adb0e42385fbe86470e3eb3d..dd70517f967424887daf3f30e5aed5b1e6f459c8` — no whitespace diagnostics observed
- `git diff --color=never 4bf0e2715c50bfe3adb0e42385fbe86470e3eb3d..dd70517f967424887daf3f30e5aed5b1e6f459c8 -- crates/client/src/ticket_role.rs crates/client/src/spawn.rs`

I did not run cargo/nix validation because this re-review was focused on blocker fixes and I avoided build commands that would write artifacts.

## 7. Residual risk

The implementation is now suitable to merge for the launcher layer. The remaining execution limitation is intentional and explicit: `inherit` can be planned but not top-level executed through this client path. Future TUI/CLI integration should surface that diagnostic clearly or require concrete role profiles until a proper inheritance-aware launch path exists.


---

<!-- event: implementation_report author: hare at: 2026-06-05T19:32:53Z -->

## Implementation report

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


---

<!-- event: close author: hare at: 2026-06-05T19:34:06Z status: closed -->

## Closed

Ticket role Pod launcher is complete and merged.

Implementation:

- `4bf0e27 feat: add ticket role pod launcher`
- `dd70517 fix: harden ticket role launch execution`
- merge commit: `3d6c1ab merge: add ticket role launcher`

Summary:

- Added `crates/client/src/ticket_role.rs` as a reusable client-level Ticket role launch layer.
- Added launch planning for fixed Ticket roles using `.yoi/ticket.config.toml`:
  - intake
  - orchestrator
  - coder
  - reviewer
  - investigator
- Kept TUI free from `pod` internals; TUI can use `client`.
- Generated first-run input as `Segment::WorkflowInvoke` plus `Segment::Text`.
- Kept Profile responsible for durable system/role behavior.
- Did not add role-level `system_instruction` support.
- Exposed unresolved `launch_prompt` refs in plans/text without treating them as system instructions.
- Added execution API using `spawn_pod`, `PodClient`, and `Method::Run` with acceptance confirmation.
- Top-level execution now rejects `profile = "inherit"` with `UnsupportedInheritProfile` rather than passing invalid `--profile inherit` semantics.
- Run delivery waits for acceptance evidence (`UserMessage`, `InvokeStart UserSend`, or `TurnStart`) and reports error/close/timeout.

Review:

- External sibling review initially requested changes for two blockers:
  1. invalid top-level execution of `inherit` profile;
  2. no first-run acceptance confirmation.
- Both blockers were fixed in `dd70517`.
- Re-review approved with no blockers.

Non-blocker follow-ups:

- Add fake-socket/client execution tests for acceptance/rejection/close/timeout behavior.
- Add aggregate prompt/list caps; current implementation bounds individual fields but not list length globally.
- TUI/CLI integration should surface `UnsupportedInheritProfile` clearly or require concrete role profiles until an inheritance-aware launch path exists.

Post-merge validation passed:

- `cargo test -p client ticket`
- `cargo test -p ticket`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `git diff --check`
- `./tickets.sh doctor`
- `nix build .#yoi --no-link`

This clears the prerequisite for `tui-ticket-role-actions`.


---
