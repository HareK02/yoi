<!-- event: migration author: tickets.sh-migration at: 2026-05-27T00:00:18Z -->

## Migrated

Migrated from tickets/tui-user-model-setup.md. No legacy review file was present at migration time.

---

<!-- event: decision author: ticket-intake at: 2026-06-08T07:29:01Z -->

## Decision

## Intake refinement: current Yoi context

This is an existing migrated Ticket; no duplicate Ticket was created. The original body remains useful for the desired wizard shape, but several migrated assumptions are stale and must be treated as superseded where they conflict with current Yoi code and project decisions.

### Current request snapshot

Add an interactive TUI setup flow that helps a first-time user choose a provider/model from the provider catalog and persist a user-level default model configuration so a normal fresh `yoi` spawn can resolve a model without manual TOML editing.

### Binding decisions / invariants

- Product entrypoint is the installed `yoi` binary, not an old standalone `tui`/`insomnia` binary. The CLI surface should be chosen under the `yoi` CLI owner boundary; the migrated `tui setup-model` spelling is only historical/placeholder text.
- Current config paths use `manifest::paths::config_dir()` with default `$XDG_CONFIG_HOME/yoi` / `$HOME/.config/yoi`, not `~/.config/insomnia`.
- Normal reusable runtime configuration is Profile-oriented. The implementation must decide, with preflight, whether this wizard writes/updates `profiles.toml`, a profile-local model fragment, or another explicit user config surface; it must not silently reintroduce the removed ambient manifest-cascade model.
- Current catalog auth hints are `AuthHint::None`, `AuthHint::ApiKey`, `AuthHint::SecretRef { ref_ }`, and `AuthHint::CodexOAuth`; the migrated `ApiKey { env: Option<String> }` flow is stale.
- Secret values must not be written into Ticket bodies, logs, diagnostics, or generated artifacts. Prefer the existing local secret-store / `yoi keys` boundary for normal provider credentials; raw `model.auth.file` remains a low-level explicit-file source, not the default UX if a safer secret-ref path is available.
- The wizard must be cancel-safe: no user config/secret writes before explicit confirmation, and failed parsing/writing must leave existing config usable.

### Implementation latitude

- The exact command name may be settled during preflight, but should fit existing `yoi` CLI semantics and help text.
- A simple vertical provider/model list is sufficient for the first implementation; search/filtering can be deferred unless preflight finds the catalog size makes it necessary.
- If only one model exists for the selected provider, skipping the model-choice step is acceptable.
- Preview may show the generated/surgical config change rather than a full diff, as long as overwrite of an existing model/default is explicit.

### Acceptance criteria

- A first-time user can launch the setup flow from the `yoi` CLI without entering normal Pod spawn by accident.
- The flow loads current `provider::catalog::load_providers()` and `load_models()` data, displays provider/model choices, and handles all current `AuthHint` variants.
- The confirmed result persists a user-level default model/profile configuration at the current Yoi config path, without storing plaintext secrets in config by default.
- Existing user config with an existing model/default is detected and requires overwrite confirmation; malformed config is reported and not rewritten.
- Esc/Ctrl-C cancel before confirmation leaves files unchanged.
- After setup, a normal fresh `yoi` spawn can resolve the selected model/profile without a model-resolve error.
- Validation includes focused Rust tests for CLI parsing/config rendering/update behavior and `nix build .#yoi` because this changes CLI/TUI/runtime resources/packaging-visible code.

### Readiness / routing signal

- readiness: spike_needed
- needs_preflight: true
- risk_flags: [cli-ux, profiles-config, secrets, auth-boundary, migration-staleness, tui]

This Ticket is ready for Orchestrator routing to preflight/spike, not direct implementation. Preflight should first settle the current user-config write target and command spelling, then either update the Ticket body or record a concise binding decision before implementation.

---

<!-- event: intake_summary author: ticket-intake at: 2026-06-08T07:29:10Z -->

## Intake summary

Updated existing Ticket 20260527-000018-tui-user-model-setup in place; no duplicate was created. The migrated request is now clarified as a Yoi CLI/TUI first-run model setup flow, with stale `tui`/`insomnia`/old AuthHint assumptions explicitly superseded in the thread. Routing readiness is spike/preflight, not direct implementation: first settle current `yoi` command spelling and the user-level config write target for Profile-oriented model defaults, then implement the wizard. needs_preflight=true; risk_flags=[cli-ux, profiles-config, secrets, auth-boundary, migration-staleness, tui].

---

<!-- event: state_changed author: ticket-intake at: 2026-06-08T07:29:10Z from: intake to: ready reason: intake_ready field: workflow_state -->

## State changed

Intake clarification completed for the existing Ticket. The Ticket is ready for Orchestrator routing to preflight/spike; implementation should not begin until the preflight decisions called out in the intake summary are recorded.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-10T07:59:32Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `yoi ticket` が queued にしました。


---

<!-- event: decision author: hare at: 2026-06-10T07:59:32Z -->

## Decision

Human authorized Queue from workspace panel notification; Orchestrator read the Ticket and will accept before implementation side effects.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-10T07:59:32Z from: queued to: inprogress reason: cli_state field: state -->

## State changed

State changed to `inprogress`.


---

<!-- event: decision author: hare at: 2026-06-10T07:59:32Z -->

## Decision

Accepting implementation after workspace inspection. Expected overlap with prompt-resource cleanup is low, so route in parallel in a dedicated worktree.


---

<!-- event: plan author: orchestrator at: 2026-06-10T09:02:02Z -->

## Plan

Routing decision: implementation_ready

Reason:
- Ticket is already `inprogress`; relation query and orchestration-plan query show no blockers.
- Existing thread marked this as needing a small preflight. Bounded code/context check resolved the implementation-shaping decisions without needing another planning round.

Evidence checked:
- Ticket body/thread/artifacts for `00001KSKBPSJG`.
- Relation records: none.
- Orchestration plan records: none before this routing pass.
- Current worktree/branch state: `.worktree/tui-model-setup-wizard` exists and has no implementation commit yet.
- Current CLI surface in `crates/yoi/src/main.rs` includes top-level `panel`, `keys`, `pod`, `ticket`, `objective`, `session`, `memory lint`; setup is not yet present.
- Profile/config surface in `crates/manifest/src/paths.rs` and `crates/manifest/src/profile.rs`; active reusable Profiles are Lua-first with user registry/config under the normal config root.
- Provider/model catalog resources under `resources/providers` and `resources/models`.

Binding decisions / invariants:
- Add an explicit one-shot setup entry point `yoi setup-model`; it must not run normal Pod startup, create sessions, attach sockets, or spawn Pods while configuring.
- Persist the result as a user default Profile through normal Profile mechanisms: user `profiles.toml` plus a generated user Profile Lua file (for example `profiles/default.lua`) under the normal config root. Do not write workspace `.yoi`, Ticket records, session history, runtime/local/secret-like files, or project records from the setup wizard.
- Do not introduce legacy manifest authority or new environment-variable configuration surfaces.
- Keep existing startup/profile semantics intact; the setup wizard only creates/updates config that normal startup already knows how to consume.
- If provider credential handling needs more than selecting/recording an existing auth hint/ref, escalate rather than inventing a new secret store flow.

Implementation latitude:
- Exact TUI layout and internal state machine organization are up to coder, bounded by the one-shot setup and persistence invariants.
- Coder may choose the generated profile name/path if it is deterministic, user-config scoped, and unambiguous from the default selector.
- Coder may keep the catalog/model choice set modest and testable for the first implementation.

Validation:
- Focused CLI/parser and setup persistence tests.
- Focused TUI/setup module tests where practical.
- `cargo fmt --check` or `cargo fmt`, `git diff --check`, focused cargo tests, and broader `cargo check`/`nix build .#yoi` before merge because this touches CLI/TUI/config/package source surfaces.

Escalate if:
- The implementation requires changing Profile registry semantics, secret-store behavior, normal Pod launch behavior, or workspace `.yoi` authority.
- Credential entry/storage cannot be limited to an existing explicit auth reference/hint.

---

<!-- event: plan author: orchestrator at: 2026-06-10T09:08:59Z -->

## Plan

Routing update: blocked_by_dependency_or_missing_authority

Reason:
- This Ticket remains `inprogress` and has an accepted implementation plan for `.worktree/tui-model-setup-wizard`, but the current Orchestrator process cannot delegate write scope to the child worktree.
- Attempted `SpawnPod` for `coder-tui-model-setup-wizard` with write scope limited to the child worktree failed: requested scope `/home/hare/Projects/yoi/.worktree/tui-model-setup-wizard` is not within the spawner effective scope.
- Retrying with broader `/home/hare/Projects/yoi` write scope also failed for the same effective-scope reason.
- Direct file write to the child worktree through the available tool scope is read-only, so the Orchestrator cannot safely implement the Ticket in this session either.

Evidence checked:
- Ticket body/thread/artifacts and relation/orchestration-plan records.
- Current CLI/profile/provider code map was boundedly inspected and the accepted plan was recorded.
- Worktree `.worktree/tui-model-setup-wizard` exists, but no coder was spawned and no implementation files were changed.

Next action:
- Continue this Ticket from a Pod/session with delegated write scope for `.worktree/tui-model-setup-wizard`, or explicitly re-route/clean up the existing inprogress worktree.
- Do not treat this as planning uncertainty; the current blocker is write/delegation authority for implementation side effects.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-10T09:16:45Z -->

## Implementation report

Implementation report

Branch/worktree: `tui-model-setup-wizard` / `.worktree/tui-model-setup-wizard`
Commits:
- `6bb023e9 merge develop into setup wizard worktree` (brought branch up to current `develop` before implementation)
- `32be6075 feat: add setup model command`

Summary:
- Added a top-level `yoi setup-model` command that enters a one-shot setup path separate from normal Pod startup.
- Added `tui::setup_model` to list catalog-backed models/providers, prompt for a default selection, and persist user Profile config under the normal user config directory.
- Persistence writes `profiles.toml` with `default = "user:default"` and `[profile.default] path = "profiles/default.lua"`, plus generated `profiles/default.lua` using the selected model ref.
- The setup path does not start/attach a Pod, create sessions, or write workspace `.yoi` records.
- Added CLI parser tests for `setup-model` and persistence tests for generated profile config.

Validation run in branch:
- `cargo fmt`
- `cargo test -p tui setup_model --lib` passed.
- `cargo test -p yoi parse_setup_model --bin yoi` passed.
- `cargo check -p yoi` passed.
- `git diff --check` passed.

Notes:
- `nix build .#yoi` was not run in the branch yet; Orchestrator should run it before merge because this touches CLI/TUI/config/package source surfaces.
- The implementation uses a simple bounded terminal setup flow rather than broad TUI refactoring.

---
