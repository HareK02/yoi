<!-- event: create author: hare at: 2026-05-29T22:28:50Z -->

Created as a follow-up to the closed manifest profiles work item after reviewing the original intent and the current built-in Nix profile shape.

---

<!-- event: plan author: planning-pod at: 2026-05-29T22:36:45Z -->

Implementation plan written to `artifacts/implementation-plan.md`. Key recommendation: introduce a typed semantic profile artifact and manifestization step, move/centralize model catalog context-window resolution so compaction can derive from model metadata, and resolve builtin profiles in-process so normal default startup does not require external `nix`.

<!-- event: plan author: hare at: 2026-05-30T00:45:10Z -->

## Plan

## Implementation direction

Use the worktree + sibling coder/reviewer flow. The investigation Pod found the main boundary issue to be concentrated in `crates/manifest/src/profile.rs`: profile artifacts are currently deserialized as `PodManifestConfig`, while built-in Nix files expose a manifest-shaped authoring API.

Decisions for the implementation pass:

- Move data-only model catalog resolution to a cycle-free place owned by `manifest` so profile manifestization can derive context-window-dependent settings. Keep live provider/client construction in `provider`.
- Introduce a semantic profile artifact format such as `insomnia.semantic-profile.v1`; top-level semantic artifacts must use `profile` metadata plus `policy`, not `manifest` / `config`.
- Resolve built-in `default` in-process so the normal default startup path does not require an external `nix` command. User/project/path Nix profiles may still require `nix eval` with clear diagnostics.
- Remove `pod.name` from built-in profiles. Direct no-arg `insomnia-pod` should preserve the current effective default Pod name `insomnia` as runtime input, while `--profile-pod-name` remains the explicit fresh-spawn override.
- Do not preserve manifest-shaped Nix profiles as the normal authoring API. `--manifest` remains the low-level concrete manifest escape hatch.
- Keep the initial semantic policy narrow: explicit model ref, explicit reasoning effort, workspace scope policy, compaction ratios/preset derived from effective context window, memory/session/web policy sufficient to reproduce current builtin behavior.
- Update docs and tests so built-in examples no longer recommend `mkManifest` or raw manifest-shaped profiles.

Implementation should remain narrow: preserve existing profile selection semantics, source-qualified selectors, ambiguity errors, TUI/client profile selection flow, and `SpawnPod.scope` authority.


---

<!-- event: decision author: hare at: 2026-05-30T01:39:47Z -->

## Decision

Implementation is paused after design discussion. The attempted direction of introducing a semantic JSON-profile-to-Manifest projection is not accepted as the specification: it risks preserving nearly the same information as Manifest and therefore failing the intended abstraction.

The current design question has been split into `profile-authoring-requirements-sync`, which records the shared requirements and open questions before choosing Lua/external Nix/another authoring language. Do not proceed with the existing child-worktree implementation until that specification is decided.


---
