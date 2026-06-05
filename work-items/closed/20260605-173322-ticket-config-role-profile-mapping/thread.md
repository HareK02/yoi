<!-- event: create author: tickets.sh at: 2026-06-05T17:33:22Z -->

## Created

Created by tickets.sh create.

---

<!-- event: decision author: hare at: 2026-06-05T17:35:08Z -->

## Decision

Decision: implement `.yoi/ticket.config.toml` as Ticket orchestration configuration with fixed Ticket role slots.

Use fixed roles, not an arbitrary Role registry:

- intake
- orchestrator
- coder
- reviewer
- investigator

The config maps these fixed Ticket roles to Profile selector strings and optional role system instruction / launch prompt / workflow refs. This keeps Profile as the Pod runtime recipe while Ticket orchestration owns the role-to-profile binding.

The first implementation should parse/validate config and wire the configured backend root into Ticket tools. It should not spawn Pods, add TUI actions, or implement a stateful workflow engine yet.

Detailed investigation and implementation plan: `artifacts/investigation-plan.md`.


---

<!-- event: plan author: hare at: 2026-06-05T17:35:08Z -->

## Plan

Plan:

1. Add Ticket config model/parser, probably in `crates/ticket/src/config.rs`, using lightweight string wrapper types for Profile/prompt/workflow refs so `ticket` does not depend on `pod`.
2. Parse `.yoi/ticket.config.toml` from a workspace root, with defaults when missing.
3. Support `[backend]` local root and fixed `[roles.*]` sections.
4. Wire the configured backend root into `crates/pod/src/feature/builtin/ticket.rs` so Ticket tools no longer hard-code `<workspace>/work-items`.
5. Add focused tests in `ticket` and `pod`.
6. Defer Pod role launching, TUI actions, and workflow-state/prompt sequencing to follow-up tickets.


---

<!-- event: decision author: hare at: 2026-06-05T18:15:02Z -->

## Decision

Decision update: remove `system_instruction` from the MVP `.yoi/ticket.config.toml` role schema.

Rationale:

- The selected Profile already owns durable role behavior and system/instruction configuration.
- Adding `system_instruction` to Ticket role config would create an unclear override/merge boundary with Profile.
- Ticket role config should bind fixed Ticket roles to Profile selectors, workflow refs, and launch prompts only.
- Launch prompts remain useful here because they are per-action first committed task/user messages, unlike role-stable system behavior.

Updated MVP role fields:

- `profile`
- optional `launch_prompt`
- optional `workflow`

If system-level role overlays are needed later, design them explicitly as a separate prompt/context ticket, likely with an overlay name and clear history/cache semantics. Do not add an implicit system instruction override in this ticket.


---

<!-- event: review author: hare at: 2026-06-05T18:45:41Z status: approve -->

## Review: approve

# External review: ticket-config-role-profile-mapping

## 1. Result: request changes

Request changes. The implementation is otherwise close to the ticket, but one validation gap fails the requested alignment with existing `SpawnPod.profile` selector rules and should be fixed before merge.

## 2. Summary of implementation

The coder commit `767870a4fbf12f942a8b270e1cc316d7f35d3ef6` adds `crates/ticket/src/config.rs` and exports it from the `ticket` crate. The new parser reads `.yoi/ticket.config.toml`, defaults missing config to `<workspace>/work-items` plus fixed role defaults, models the fixed roles `intake`, `orchestrator`, `coder`, `reviewer`, and `investigator`, and stores profile / launch prompt / workflow references as lightweight strings without introducing `pod` or `manifest` dependencies.

The Pod built-in Ticket feature now loads `TicketConfig` from the Pod working directory, uses the configured backend root for `LocalTicketBackend`, and refuses to register Ticket tools when the config is malformed or the backend root is unusable. The implementation does not add Pod spawning, TUI actions, workflow state, system-instruction overlays, role registries, external trackers, or scheduler behavior.

## 3. Requirement-by-requirement assessment

- `.yoi/ticket.config.toml` path and schema: mostly satisfied. The parser uses the fixed path `.yoi/ticket.config.toml`, supports `[backend] kind/root`, and uses fixed `[roles.<role>]` sections with `profile`, optional `launch_prompt`, and optional `workflow`.
- Fixed roles only: satisfied. Unknown role names are rejected during config resolution.
- No `system_instruction` role field: satisfied. `deny_unknown_fields` rejects it and a test checks this.
- Missing config defaults: satisfied. Missing file returns local backend `<workspace>/work-items`, all role profiles `inherit`, no launch prompts, and the documented workflow defaults.
- Relative backend roots: satisfied. Relative roots are joined to the workspace root.
- Backend directories not auto-created: satisfied in the Pod adapter path. The adapter canonicalizes/checks the root and required `open/`, `pending/`, and `closed/` directories before registering tools.
- Unknown roles/fields and malformed refs: mostly satisfied, but see blocker below for an accepted path-like profile selector that `SpawnPod.profile` rejects.
- Crate dependency boundary: satisfied. `ticket` adds `toml` but does not depend on `pod` or `manifest`; profile/prompt/workflow refs remain string wrappers.
- Pod adapter configured root / fail-closed behavior: satisfied. Config parse errors and unusable roots produce diagnostics and no Ticket tools are registered.
- HostAuthority root consistency: acceptable but imperfect. The backend uses the canonicalized usable root, while `HostAuthority::TicketBackend { root }` is built from the pre-canonicalized configured path; see follow-up.
- Explicit non-goals: satisfied. I found no added Pod spawning, TUI action, workflow engine, prompt injection, Profile semantic change, `system_instruction` overlay, arbitrary role registry, storage rename, external tracker, or scheduler work.
- `Cargo.lock` / `package.nix`: changes are limited to adding the existing workspace `toml` dependency to `ticket` and updating the Nix cargo hash. That is necessary and looks safe.
- Tests: broadly cover missing/full/partial config, unknown role/field, relative root, unsupported backend kind, malformed profile path, and Pod adapter root/no-register behavior. They do not cover the blocker case below.

## 4. Blockers

1. `ProfileSelectorRef` accepts `legacy.nix`/`*.nix` as a valid role profile selector, but `SpawnPod.profile` explicitly rejects `*.nix` as path-like.

   The ticket requires role profile selector syntax to stay aligned with existing `SpawnPod/profile` selectors where possible, and the review checklist asks that malformed refs be rejected or clearly reported. `crates/pod/src/spawn/tool.rs` rejects path-like profile values including `legacy.nix`, while `crates/ticket/src/config.rs` currently rejects `path:`, dot-prefixed values, values containing `/`, and `*.lua`, but not `*.nix`. Because role config values are meant to be later usable by role launch code, accepting a selector that the existing launch boundary rejects is a config-validation failure.

   Expected fix: reject `*.nix` in `ProfileSelectorRef::new` and add a focused test alongside the existing malformed ref test.

## 5. Non-blockers / follow-ups

- `HostAuthority::TicketBackend { root }` is derived from `self.backend_root.display()` before canonicalization, while the actual `LocalTicketBackend` is built from `usable_root` after `canonicalize()`. This can make the granted/audited authority root differ from the root used by tools when the configured path includes `..` components or symlinks. The current implementation still requires matching host authority on the contributed tools and fail-closes on unusable roots, so I am not blocking on it, but the adapter should prefer a validated/canonical authority root where practical.
- The Pod adapter test for configured backend root checks feature root selection and tool registration count. It does not execute a tool against the configured root. The code path is straightforward (`LocalTicketBackend::new(usable_root)`), so this is acceptable, but an execution-level regression test would be stronger.

## 6. Validation assessed or rerun

Rerun/read-only checks:

- `cd /home/hare/Projects/yoi/.worktree/ticket-config-role-profile-mapping && git diff --stat develop...HEAD`
- `cd /home/hare/Projects/yoi/.worktree/ticket-config-role-profile-mapping && git diff --name-status develop...HEAD`
- `cd /home/hare/Projects/yoi/.worktree/ticket-config-role-profile-mapping && git diff --check develop...HEAD`
- `cd /home/hare/Projects/yoi/.worktree/ticket-config-role-profile-mapping && git show --stat --oneline --decorate 767870a4fbf12f942a8b270e1cc316d7f35d3ef6`

Assessed by inspection:

- Ticket requirements, investigation/plan, and thread.
- `crates/ticket/src/config.rs`
- `crates/ticket/src/lib.rs`
- `crates/ticket/Cargo.toml`
- `crates/pod/src/feature/builtin/ticket.rs`
- `Cargo.lock`
- `package.nix`
- Relevant existing `SpawnPod.profile` selector validation in `crates/pod/src/spawn/tool.rs`.

Not rerun: `cargo test`, `cargo check`, `cargo fmt --check`, `./tickets.sh doctor`, or `nix build`. The review request allowed focused read-only validation, and rerunning these would write build/test artifacts outside the review artifact path in this scoped sibling review.

## 7. Residual risk

After the `*.nix` selector rejection is fixed, residual risk is mainly around future launch integration: prompt/workflow refs are intentionally lightweight strings and will need runtime validation when the role launcher resolves them. The configured backend root is wired into the current Ticket tools, but authority-root canonicalization should be tightened before relying on HostAuthority root strings for security/audit semantics beyond this feature gate.

---

## Re-review of blocker fix: 8fab67b

### Result: approve

The blocker is resolved, and I found no new blocker in the focused fix commit.

### Assessment

- `ProfileSelectorRef::new` now rejects values ending in `.nix` alongside other path-like selectors (`path:`, dot-prefixed selectors, slash-containing selectors, and `.lua`). This aligns the Ticket role profile config validation with the existing `SpawnPod.profile` path-selector rejection boundary for the reported case.
- A focused test, `nix_profile_selector_refs_are_rejected`, was added for `profile = "legacy.nix"` and asserts that the config load fails with the path-selector rejection message.
- The fix is limited to `crates/ticket/src/config.rs` and does not introduce source-boundary, runtime behavior, dependency, or scope expansion changes.

### Validation assessed

Rerun/read-only checks:

- `cd /home/hare/Projects/yoi/.worktree/ticket-config-role-profile-mapping && git show --stat --oneline HEAD && git diff develop...HEAD -- crates/ticket/src/config.rs`
- `cd /home/hare/Projects/yoi/.worktree/ticket-config-role-profile-mapping && git show --stat --oneline HEAD && git show --unified=8 -- crates/ticket/src/config.rs`

### Blockers

None.


---

<!-- event: implementation_report author: hare at: 2026-06-05T18:45:41Z -->

## Implementation report

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


---

<!-- event: close author: hare at: 2026-06-05T18:48:15Z status: closed -->

## Closed

Ticket config role profile mapping is complete and merged.

Implementation:

- `767870a ticket: add workspace ticket config`
- `8fab67b ticket: reject nix profile selectors`
- merge commit: `9910df4 merge: add ticket config roles`

Summary:

- Added `.yoi/ticket.config.toml` support through `crates/ticket/src/config.rs`.
- Added fixed Ticket roles only:
  - `intake`
  - `orchestrator`
  - `coder`
  - `reviewer`
  - `investigator`
- Added role config fields:
  - `profile`
  - optional `launch_prompt`
  - optional `workflow`
- Did not add role-level `system_instruction`; durable role/system behavior remains owned by the selected Profile.
- Added backend config for local Ticket storage:
  - `kind = "local"`
  - `root = "work-items"`
- Missing config defaults to local `<workspace>/work-items`, all role profiles `inherit`, no launch prompts, and documented workflow defaults.
- Unknown roles/fields, unsupported backend kinds, malformed refs, path-like profile refs, `.lua`, and `.nix` profile refs are rejected.
- Wired the configured backend root into `crates/pod/src/feature/builtin/ticket.rs`.
- Preserved fail-closed/no-register behavior for malformed config or unusable backend roots.
- Kept dependency direction clean: `pod -> ticket`; `ticket` does not depend on `pod` or `manifest`.

Review:

- External sibling review initially requested one blocker fix: reject `*.nix` profile selectors to align with `SpawnPod.profile` validation.
- Coder fixed it in `8fab67b` and added a focused `legacy.nix` rejection test.
- Re-review approved with no blockers.

Non-blocker follow-ups:

- Normalize `HostAuthority::TicketBackend { root }` and canonical backend root consistently before relying on root strings for stricter security/audit comparison.
- Add an execution-level Pod adapter test that runs a Ticket tool against the configured backend root.
- Later role launcher should resolve Profile selectors/prompt refs/workflow refs at runtime and commit launch prompts as user/task messages.

Post-merge validation passed:

- `cargo test -p ticket`
- `cargo test -p pod ticket --lib`
- `cargo test -p pod feature --lib`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `git diff --check`
- `./tickets.sh doctor`
- `nix build .#yoi --no-link`


---
