<!-- event: create author: tickets.sh at: 2026-05-29T20:55:40Z -->

## Created

Created by tickets.sh create.

---

<!-- event: decision author: hare at: 2026-05-30T02:53:19Z -->

## Decision

Clarified selector semantics:

- `default` / omitted means resolve the effective child default profile.
- `<slug>` / source-qualified selectors mean resolve a discovered role profile.
- `inherit` means derive reusable child config from the spawner's resolved Manifest.

`inherit` is explicitly not the same as reusing the Profile source that created the parent. It extracts reusable fields from the parent resolved Manifest and still replaces runtime-bound/authority fields such as `pod.name` and concrete `scope.allow` with the SpawnPod inputs. Reusing the parent's original Profile source can be considered later as a separate feature if needed.


---

<!-- event: plan author: hare at: 2026-05-30T04:54:02Z -->

## Plan

## Preflight implementation plan

Classification: implementation-ready.

No product/API decision is needed before coding. The ticket already fixes the important semantics: omitted/default uses the effective child default profile, `inherit` derives reusable config from the spawner's resolved Manifest, named/source-qualified selectors resolve discovered profiles, path selectors are rejected, and `SpawnPod.scope` remains the only delegated capability.

Important implementation notes:
- Do not rely on process `current_dir()` for SpawnPod profile discovery. Use the Pod cwd (`spawner_pwd`) explicitly by adding/exposing a resolver helper that resolves from a registry discovered for that cwd.
- Resolve profiles and build child config before pod-registry reservation where possible, so invalid profile selectors do not mutate registry/scope.
- `inherit` means derive from the parent resolved Manifest, not from the parent's original Profile source.
- Path-like values, `path:<...>`, `.lua`/legacy suffix selectors, and absolute/relative paths must fail closed in `SpawnPod.profile`.
- Existing hidden `--spawn-config-json` remains the internal handoff; do not exec child with `--profile`.
- Existing prompt-loader source limitations are out of scope; preserve current behavior.

Current code map:
- `crates/pod/src/spawn/tool.rs`: `SpawnPodInput`, static description, spawn lifecycle, `build_spawn_config_json`.
- `crates/pod/src/controller.rs`: `register_pod_tools`, currently snapshots parent model/trace and registers spawn tools.
- `crates/manifest/src/profile.rs`: `ProfileDiscovery`, `ProfileRegistry`, `ProfileSelector`, `ProfileResolver`.
- `crates/manifest/src/config.rs`: `PodManifestConfig`, merge/resolve/defaults.
- `crates/pod/src/main.rs`: hidden `--spawn-config-json` loading takes precedence and uses builtins-only prompt loader.
- `crates/pod/src/prompt/catalog.rs` and `resources/prompts/internal.toml`: central prompt catalog for templated tool description.

Implementation phases:
1. Add manifest profile resolver helper for registry/cwd-explicit selection.
2. Add `SpawnPodInput.profile` and a SpawnPod-specific selector parser for `default`, `inherit`, and registry selectors only.
3. Add shared available-profile formatter for tool description and error diagnostics.
4. Move SpawnPod tool description into prompt catalog/minijinja and render it during tool registration; discovery failures should render diagnostics, not fail Pod startup.
5. Build child `PodManifestConfig` from selected profile Manifest or inherited parent Manifest, replacing `pod.name`, replacing `scope.allow`, clearing `scope.deny`, and optionally overriding only `worker.instruction`.
6. Preserve existing lifecycle: registry reservation/rollback, scope revocation, spawned registry write, callback wiring, child socket wait, initial `Method::Run` confirmation.
7. Update docs/workflows with `project:coder`, `project:reviewer`, optional `project:orchestrator`, and `inherit` examples.

Critical risks:
- Do not merge profile/inherited scope with explicit SpawnPod scope; explicit scope replaces capability.
- Do not call CLI-style profile parser in a way that allows path profiles through SpawnPod.
- Description and diagnostic profile lists should share formatting.
- Prompt catalog key coverage is build-time enforced.

Validation plan:
- Unit tests for selector parsing, formatter, config builder override/replacement behavior.
- Manifest tests for cwd/registry-explicit resolver helper.
- Prompt catalog rendering test.
- SpawnPod integration tests for omitted default, inherit, project profile, invalid selector pre-reservation failure, ambiguity suggestions, and scope replacement.
- Run `cargo test -p manifest profile`, `cargo test -p pod spawn_pod`, relevant prompt catalog tests, `cargo fmt --check`, and `./tickets.sh doctor`.


---

<!-- event: review author: hare at: 2026-05-30T05:11:43Z status: request_changes -->

## Review: request changes

Request changes.

The implementation direction appears sound, and the reviewer did not find a concrete authority-expansion or lifecycle regression. However, the work item acceptance criteria require focused tests for the new SpawnPod profile semantics, and the submitted tests mostly cover selector parsing plus existing lifecycle tests forced to `profile = "inherit"`.

Required fixes:
- Add SpawnPod default-profile coverage proving omitted `profile` resolves the effective registry default.
- Add a source-qualified profile coverage case, e.g. `project:reviewer`, proving role config from the selected profile reaches the generated `--spawn-config-json`.
- Add `inherit` config coverage proving reusable parent fields are copied while `pod.name`, `scope.allow`, and `scope.deny` are replaced.
- Add explicit `instruction` override coverage proving only `worker.instruction` changes.
- Add invalid / ambiguous / no-default diagnostics coverage proving the available-selector block appears.
- Add profile scope replacement coverage proving profile/inherited scope cannot expand delegated scope.

Non-blocking follow-ups:
- Available profile list currently emits source-qualified selectors only; future refinement may mention unqualified names when unambiguous.
- Workflow examples can later be updated to use explicit `project:coder` / `project:reviewer` selectors.

Validation note:
- `cargo test -p pod spawn_profile --no-default-features` currently only proves parser behavior, not profile resolution or child config construction.


---

<!-- event: review author: hare at: 2026-05-30T05:18:48Z status: approve -->

## Review: approve

Approve.

The requested test coverage gaps are satisfied by `b94891e`. The new tests directly cover the previously missing SpawnPod profile/config-building behaviors, and the added seam remains private to the module/test context rather than exposing a new production API.

Coverage verified:
- Omitted/default selector resolves the effective registry default.
- Source-qualified `project:reviewer` role config reaches generated hidden spawn config / child config.
- `inherit` copies reusable parent fields while replacing `pod.name`, `scope.allow`, and `scope.deny`.
- Explicit `instruction` override changes only `worker.instruction` while preserving other selected role fields.
- Invalid / ambiguous / no-default diagnostics include available-selector guidance.
- Profile and inherited scope are replaced by delegated scope and cannot expand child authority.

Remaining blockers: none.

Non-blocking follow-ups:
- Default-selector coverage exercises `SpawnProfileSelector::Default` directly while omitted parse coverage is separate; acceptable.
- Diagnostics parser test manually composes the invalid-selector suffix instead of going through full `execute`; acceptable because production append logic is simple and builder diagnostics are covered directly.

Validation reviewed:
- Coder: `cargo fmt --check`, `cargo test -p manifest profile`, `cargo test -p pod`, `cargo test -p pod --no-default-features` passed.
- Reviewer spot-ran: `cargo test -p pod profile`, `cargo test -p manifest profile`, `cargo test -p pod --no-default-features` passed.

Final verdict: approve.


---
