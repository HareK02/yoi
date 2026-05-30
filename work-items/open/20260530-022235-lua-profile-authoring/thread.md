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
