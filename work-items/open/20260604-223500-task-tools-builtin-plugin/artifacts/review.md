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
