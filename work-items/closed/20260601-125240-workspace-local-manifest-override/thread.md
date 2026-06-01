<!-- event: create author: tickets.sh at: 2026-06-01T12:52:40Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-06-01T12:53:58Z -->

## Plan

# Delegation intent: workspace-local manifest override

Intent:
- Implement a workspace-local manifest override file that overlays Profile-derived normal startup configuration.

Requirements:
- Support `.yoi/override.local.toml` as the local ignored override file.
- Detect the nearest workspace override relative to the active workspace/project base used for profile discovery.
- Apply the override as a `PodManifestConfig` layer after Profile resolution and before final `PodManifest` validation/snapshot persistence.
- Preserve explicit `--manifest <path>` as a one-file escape hatch; do not apply workspace overrides in explicit manifest mode unless you stop and report a strong reason.
- Resolve relative paths in the override file from `.yoi/` / the override file parent directory.
- Add provenance/diagnostic information where the existing manifest/profile resolution surfaces can carry it without broad refactoring.
- Add focused tests for discovery, merge order, path base, and explicit-manifest behavior.
- Update docs only if the behavior is durable and useful to document.

Invariants:
- Do not reintroduce ambient `.yoi/manifest.toml` cascade as normal startup.
- Do not put runtime-bound fields into reusable Profiles.
- Keep explicit manifest mode low-level and predictable.
- Do not read ignored secret-like file contents.
- Do not edit unrelated tickets or parent workspace files.

Non-goals:
- Do not support multiple cascading override files unless the existing workspace discovery naturally requires it.
- Do not implement a new profile language.
- Do not change dependency versions.
- Do not close the ticket or merge the worktree.

Escalate if:
- The correct workspace base is ambiguous between TUI launch, `yoi pod`, and `SpawnPod`.
- Provenance support requires changing persisted manifest schema in a broad way.
- Supporting `.yoi/override.toml` as trackable state seems necessary.
- Tests require real spawned process E2E coverage.

Validation:
- Run focused manifest/profile tests, preferably `cargo test -p manifest` plus any touched `pod` tests.
- Run `./tickets.sh doctor`, `git diff --check`, and `nix build .#yoi` if feasible in the worktree.
- Record any skipped validation with rationale.


---

<!-- event: implementation_report author: hare at: 2026-06-01T13:05:58Z -->

## Implementation report

# Implementation report

Implemented workspace-local manifest override support.

## Behavior

- Normal Profile/default resolution now searches upward from the resolver workspace base for the nearest `.yoi/override.local.toml`.
- The override is parsed as a `PodManifestConfig` layer, resolved relative to its parent `.yoi/` directory, and merged after builtin defaults plus the selected Profile but before final `PodManifest` validation and snapshot serialization.
- Resolved profile provenance now records the applied override path in `manifest.profile.workspace_override`.
- Explicit `--manifest <path>` mode remains a single-file escape hatch and does not apply workspace-local overrides.
- Workspace-local overrides are rejected if they set `pod.name`, keeping Pod identity runtime-bound.

## Validation

- `cargo test -p manifest workspace_local_override -- --nocapture`
- `cargo test -p pod manifest_mode_does_not_apply_workspace_local_override -- --nocapture`
- `cargo test -p manifest -p pod`
- `./tickets.sh doctor`
- `git diff --check`
- `nix build .#yoi`

All completed successfully.


---

<!-- event: review author: hare at: 2026-06-01T13:19:35Z status: approve -->

## Review: approve

# Review: workspace-local manifest override

Reviewer Pod: `workspace-override-reviewer-20260601`

## Result

Approved. No blockers found.

## Findings

The implementation satisfies the intent:

- `.yoi/override.local.toml` is discovered as the workspace-local override.
- The override applies only to normal Profile/default resolution.
- Explicit `--manifest <path>` remains a single-file escape hatch and does not load the workspace override.
- Merge order is builtin/default + selected Profile, then workspace override, then final `PodManifest` validation.
- Relative paths in the override are resolved from the override file parent `.yoi/` directory.
- `.yoi/manifest.toml` cascade was not reintroduced.
- Overrides that set `pod.name` are rejected.
- Provenance is recorded narrowly through `manifest.profile.workspace_override`.

## Follow-up handled

Reviewer requested non-blocking coverage for the case where both parent and nested `.yoi/override.local.toml` exist. Coder added commit `8f98785 test: cover nearest workspace override`, which verifies the nearest nested override wins and checks provenance.

## Validation evidence

Coder reported:

- `cargo test -p manifest workspace_local_override -- --nocapture`
- `cargo test -p pod manifest_mode_does_not_apply_workspace_local_override -- --nocapture`
- `cargo test -p manifest -p pod`
- `./tickets.sh doctor`
- `git diff --check`
- `nix build .#yoi`

Parent/orchestrator reran after merge:

- `cargo test -p manifest workspace_local_override -- --nocapture`
- `cargo test -p pod manifest_mode_does_not_apply_workspace_local_override -- --nocapture`
- `./tickets.sh doctor`
- `git diff --check`
- `nix build .#yoi`
- `./result/bin/yoi pod --help`

## Residual risk

`ProfileResolver::resolve()` still discovers named/default profiles from process cwd while `with_workspace_base(...)` controls scope and override discovery. This was judged non-blocking for the intended CLI/Profile/default/SpawnPod paths, but future callers should avoid assuming `with_workspace_base` also binds registry discovery.


---

<!-- event: close author: hare at: 2026-06-01T13:19:36Z status: closed -->

## Closed

Implemented workspace-local .yoi/override.local.toml as an ignored manifest override layer for normal Profile/default startup. Explicit --manifest remains override-free; focused tests and nix build passed.


---
