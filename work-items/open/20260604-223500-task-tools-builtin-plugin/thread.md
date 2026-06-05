<!-- event: create author: tickets.sh at: 2026-06-04T22:35:00Z -->

## Created

Created by tickets.sh create.

---

<!-- event: decision author: hare at: 2026-06-04T22:35:48Z -->

## Decision

# Decision: Task tools as trial built-in plugin

Use the Task tool group as a trial of the Plugin/Feature boundary, but keep the scope to a built-in plugin module.

Rationale:

- `TaskCreate` / `TaskUpdate` / `TaskGet` / `TaskList` are already small and state-bounded around `TaskStore`.
- They do not need external plugin loading, network, filesystem, secrets, model notification authority, or custom UI.
- The feature registry slice already routes them through the registry; the remaining useful trial is to extract the concrete Task feature into a clean module boundary that future built-in plugins can copy.

This ticket should not introduce a package loader or sandbox. It should validate the public API shape by making Task tools look like a normal built-in plugin/feature contribution: descriptor-declared tools, no host authorities, install through registry, normal ToolRegistry/PreToolCall behavior preserved.


---

<!-- event: plan author: hare at: 2026-06-04T23:50:50Z -->

## Plan

# Delegation intent: Task tools built-in feature module extraction

## Intent

Extract the Task tool group from the inline `pod::feature` registry implementation into a clean built-in internal feature module. This is a trial of the contribution-only built-in module path, not external plugin loading and not the external-plugin authority model.

## Worktree / branch

Use the prepared task name:

- worktree: `/home/hare/Projects/yoi/.worktree/task-tools-builtin-module`
- branch: `work/task-tools-builtin-module`

## Requirements

- Move the concrete Task feature implementation out of the generic registry/types file.
  - Current implementation is in `crates/pod/src/feature.rs` under `pub mod builtin`.
  - Candidate target: `crates/pod/src/feature/builtin/task.rs` or equivalent.
  - If converting `feature.rs` into a module directory is too large, use a small adjacent module with a clean public boundary, but keep the registry/types file focused on generic feature machinery.
- Expose a clear construction function such as:
  - `task_tools_feature(task_store: tools::TaskStore) -> impl FeatureModule`
- Preserve TaskStore sharing semantics.
  - The Pod host still creates/owns the Pod-lifetime/session `TaskStore`.
  - The Task feature receives the handle from the host constructor.
  - No TaskStore persistence/lifecycle change.
- Preserve exact tool behavior and model-visible metadata for:
  - `TaskCreate`
  - `TaskUpdate`
  - `TaskGet`
  - `TaskList`
- Ensure the Task feature descriptor declares exactly those four tools and requests no host authorities.
- Preserve descriptor-approved contribution reconciliation and once-materialized tool identity behavior.
- Keep normal ToolRegistry / PreToolCall permission behavior.
- Update imports/call sites so `Controller` uses the extracted built-in module.
- Add/update focused tests that prove Task tools install through the extracted built-in feature module and require no host authorities.
- Keep code comments/test names clear that this is a built-in/internal feature module reference pattern, not external plugin loading.

## Important boundary

Do not solve the broader authority-model split in this ticket. That is tracked separately by `feature-api-authority-separation`.

For this ticket:

- requested authorities for Task tools should stay empty;
- do not add or remove host authorities;
- do not reintroduce contribution-authority variants such as `ContributeTool`, `RunBackgroundTask`, or `ProvideService`;
- do not implement descriptor/package approval lock files.

## Non-goals

- External plugin loading.
- Plugin package format.
- WASM/runtime sandboxing.
- WorkItem or MCP integration.
- Moving Memory or Pod management.
- Reworking all built-in tool groups.
- Changing Task tool names, schemas, descriptions, outputs, or task reminder/compaction behavior.

## Suggested files

- `crates/pod/src/feature.rs`
- `crates/pod/src/lib.rs`
- `crates/pod/src/controller.rs`
- `crates/tools/src/task.rs`
- `crates/tools/src/lib.rs`
- existing feature tests in `crates/pod/src/feature.rs`

## Validation

Run at least:

- focused Task feature tests added/updated
- `cargo test -p pod --lib feature::tests --no-fail-fast` or equivalent focused feature tests
- `cargo test -p pod --lib`
- `cargo test -p llm-worker --lib`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `./tickets.sh doctor`
- `git diff --check`

Run `nix build .#yoi` if feasible.

## Completion report

Report:

- worktree path / branch
- commit hash
- changed files
- new module path / public constructor
- evidence Task feature descriptor has exactly four tools and no requested authorities
- behavior preservation notes
- tests/validation results
- unresolved risks/follow-ups
- whether ready for external review


---

<!-- event: review author: hare at: 2026-06-05T00:04:53Z status: approve -->

## Review: approve

# Review: task-tools-builtin-plugin

## 1. Result

approve

## 2. Summary of implementation

The implementation extracts the Task tool built-in wiring out of `pod::feature` generic machinery into `pod::feature::builtin::task`. The new module exposes `task_tools_feature(task_store: tools::TaskStore) -> impl FeatureModule`, declares the four Task tools through a builtin `FeatureDescriptor`, requests no host authorities, and constructs the concrete `ToolContribution`s from the host-provided `TaskStore` via the existing `tools::task_tools` constructor.

Controller registration now builds a `FeatureRegistry` with `builtin::task::task_tools_feature(pod.task_store())` and installs it through the existing `Pod::install_features` path, preserving normal ToolRegistry installation and PreToolCall behavior.

## 3. Requirement-by-requirement assessment

- Task tools are extracted from generic registry/types code into a clean built-in internal feature module: satisfied. `crates/pod/src/feature.rs` no longer contains the concrete Task feature constructor; the concrete implementation lives under `crates/pod/src/feature/builtin/task.rs`.
- New module path / constructor is clean and usable as a reference internal module pattern: satisfied. `pod::feature::builtin::task::task_tools_feature` is small, explicit, and uses the existing descriptor/contribution API without special cases.
- `pod::feature` remains focused on generic feature machinery, not concrete Task feature implementation: satisfied. The generic file retains registry/descriptor/contribution mechanics and tests; concrete Task wiring is moved out.
- Descriptor declares exactly `TaskCreate`, `TaskUpdate`, `TaskGet`, `TaskList`: satisfied. The descriptor declaration contains exactly those four tool names.
- Requested host authorities are empty: satisfied. The feature uses `FeatureDescriptor::builtin(...)` and does not add requested authorities; tests assert the list is empty.
- TaskStore sharing semantics are preserved: satisfied. The Pod/session `TaskStore` is still obtained from `pod.task_store()` at registration time and passed into the feature constructor; the feature passes that same store into the existing `tools::task_tools` constructor. No lifecycle, persistence, or reminder behavior is changed.
- Task tool names, schemas, descriptions, outputs, and normal ToolRegistry / PreToolCall path are unchanged: satisfied. Concrete tool construction remains in `tools::task_tools`; registration still materializes `ToolContribution`s into the same worker ToolRegistry path via `Pod::install_features`.
- Descriptor-approved contribution reconciliation and once-materialized tool identity behavior remain intact: satisfied. The extraction uses the existing `FeatureRegistryBuilder` / `FeatureContribution` reconciliation path. Added tests cover the descriptor/tool-name reconciliation and installed tool identity for the Task feature.
- No contribution-authority variants or broader authority model changes are introduced: satisfied. No new authority variants or authority model changes appear in the diff.
- No external plugin loading/package/WASM/WorkItem/MCP/UI/dialog changes are introduced: satisfied. The diff is limited to the Pod feature module structure, controller registration, and related tests.
- Tests cover the extraction adequately: satisfied for the intended extraction scope. Tests assert the builtin descriptor, empty authorities, installed tool identities/order, and descriptor rejection for undeclared tools. Existing task tool behavior remains owned by the unchanged `tools` crate implementation.

## 4. Blockers

None.

## 5. Non-blockers / follow-ups

None.

## 6. Validation assessed or rerun

Assessed:

- Read ticket, delegation intent, and authority-split context.
- Reviewed `git diff develop...HEAD` for coder commit `f394f15`.
- Inspected the new builtin Task feature module, controller registration, generic feature registry code, and relevant tests.

Rerun:

- `cd /home/hare/Projects/yoi/.worktree/task-tools-builtin-module && git diff --check develop...HEAD` — passed.
- `cd /home/hare/Projects/yoi/.worktree/task-tools-builtin-module && cargo fmt --check` — passed.
- `cd /home/hare/Projects/yoi/.worktree/task-tools-builtin-module && ./tickets.sh doctor` — passed.

Not rerun:

- Full `cargo test` / `nix build .#yoi` were not rerun during this external sibling review; this review stayed within focused read-only validation commands.

## 7. Residual risk

Residual risk is low. The main unverified risk is ordinary integration/build regression outside the focused extraction surface because full test and Nix package validation were not rerun here. The code diff itself preserves the existing Task tool constructor and installation path, so behavioral risk appears minimal.


---
