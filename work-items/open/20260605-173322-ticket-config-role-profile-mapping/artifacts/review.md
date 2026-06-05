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
