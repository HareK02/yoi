<!-- event: create author: tickets.sh at: 2026-05-30T02:22:35Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-05-30T02:23:16Z -->

## Plan

## Ticket preflight

Classification: implementation-ready after implementation Pod plan review.

Requirements are synchronized enough to ask an implementation Pod for a concrete plan, but not enough to skip planning. The Pod must plan first and wait for orchestrator approval before coding.

Current critical risks:
- Recreating Manifest authoring under another name. Profile may be manifest-like, but runtime-bound and authority-bearing fields (`pod.name`, concrete `scope.allow`, resolved paths, secret material, runtime state) must be rejected or diagnosed.
- Exposing `mkManifest` as the public normal API. The normal boundary should be `profile` / `insomnia.profile`.
- Implementing uncontrolled Lua `require`, standard library access, or installed resource path imports.
- Breaking existing profile selection semantics or resolved Manifest snapshot persistence.
- Letting Profile express capability expansion instead of scope intent/policy checked by runtime/delegation.

Current plan gate:
- Implementation Pod must produce a plan covering dependency/crate placement, sandbox, module loading, return contract, Profile data model, resolver integration, builtin/default migration, diagnostics, and tests.
- If the plan respects the boundary, the orchestrator may authorize implementation in the same Pod/worktree.


---

<!-- event: decision author: hare at: 2026-05-30T02:26:32Z -->

## Decision

## Implementation plan accepted with constraints

The implementation Pod produced a plan for Lua-based reusable Profile authoring. The plan is accepted as the implementation direction, with these constraints:

- Lua is the primary authoring path for this ticket; Nix is not the primary profile layer.
- Do not keep legacy `.nix` profile evaluation just for compatibility if it complicates the design. Prefer removing/disabling Nix profile resolution from the normal profile selector path, while leaving `--manifest` as the explicit low-level escape hatch.
- Builtin/default must resolve from Lua/Profile or Rust in-process data without external `nix`.
- Public authoring boundary is `profile` / `require("insomnia.profile")`, not public `mkManifest`.
- Controlled `require` is part of the implementation: host virtual `insomnia.*` modules and profile-local modules only; no installed resource path imports.
- Profile may be manifest-like, but runtime-bound or authority-bearing fields such as `pod.name`, concrete `scope.allow`/`scope.deny`, resolved paths, sockets, runtime state, and raw secret material must be rejected or clearly diagnosed.
- Scope in Profile is intent/policy only; concrete authority is resolved against runtime/delegation inputs.
- Model/context-derived compaction can use Lua locals and/or helper policy such as `compact.ratio`, not Nix recursive sets.
- Preserve profile selection semantics where still meaningful: default/builtin/user/project/source-qualified/path selectors and persisted resolved Manifest snapshots.

Implementation plan summary:

- Add embedded Lua evaluation in `crates/manifest` using a vendored Lua crate such as `mlua` if dependency/license/build characteristics are acceptable.
- Add a Profile data model that is a reusable manifest-like recipe template and converts into a concrete `PodManifest` only through resolver runtime inputs.
- Add sandboxed Lua evaluation with denied `os`, `io`, `debug`, unrestricted `package`, `dofile`, `loadfile`, and uncontrolled loaders.
- Add host-provided virtual modules such as `insomnia`, `insomnia.profile`, `insomnia.models`, `insomnia.compact`, and `insomnia.scope`.
- Add profile-local controlled `require` with canonical path checks, module cache, and cycle diagnostics.
- Migrate builtin/default from `resources/nix/profiles/default.nix` to a Lua/Profile source or in-process equivalent.
- Add focused tests for builtin/default without external nix, host modules, local require, sandbox denial, invalid Manifest-shaped returns, scope intent resolution, and selector semantics.


---
