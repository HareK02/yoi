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

<!-- event: review author: hare at: 2026-05-30T02:58:55Z status: approve -->

## Review: approve

Approve.

The implementation shifts normal profile authoring from Nix-primary evaluation to embedded Lua/Profile authoring. The public authoring boundary is `profile` / `require("insomnia.profile")`; builtin/default resolves without external `nix`; controlled `require` supports host virtual modules and profile-local modules; unsafe Lua facilities are denied; runtime-bound and authority-bearing Manifest fields are rejected; and `--manifest` remains the explicit low-level escape hatch.

Blocker findings: none.

Non-blocking follow-ups:
- `docs/pod-factory.md` still contains old ambient manifest cascade wording and should be refreshed later.
- Unsupported `.nix` entries in user/project `profiles.toml` can still appear as selectable entries before failing at resolution with the intended diagnostic.
- Builtin profile directory scanning currently treats every top-level `.lua` as a profile, so future helper files should use a convention that avoids accidental selection.
- Nested reused Manifest structs remain more lenient about unknown fields than Profile top-level validation; stronger profile-specific nested diagnostics can be considered later.

SpawnPod integration timeout assessment:
- The failing `spawn_pod_delegates_scope_and_sends_run` timeout appears unrelated to Lua profile authoring. SpawnPod hidden `--spawn-config-json` takes the direct manifest config path before profile/manifest CLI resolution, and does not invoke ProfileResolver/Lua discovery. Track separately if it remains reproducible.

Validation reviewed:
- Coder: `cargo fmt`, `git diff --check`, `cargo test -p manifest`, `cargo test -p client -p tui`, `cargo test -p pod --lib --bins` passed.
- Reviewer: `cargo test -p manifest` passed, 119 tests.

Final verdict: approve.


---

<!-- event: close author: hare at: 2026-05-30T02:59:55Z status: closed -->

## Closed

---
id: 20260530-022235-lua-profile-authoring
slug: lua-profile-authoring
title: Implement reusable Lua profile authoring
status: closed
kind: task
priority: P1
labels: [manifest, profiles, lua, architecture]
created_at: 2026-05-30T02:22:35Z
updated_at: 2026-05-30T02:59:54Z
assignee: null
legacy_ticket: null
---

## Background

The previous `semantic-nix-profiles` direction is paused. Nix-specific authoring raised portability, evaluator, and API-injection problems, and the attempted semantic JSON projection risked becoming a near-Manifest copy rather than a useful profile abstraction.

The current direction is to stop making Nix the primary profile authoring layer and implement reusable Profile authoring first. Lua is the current pragmatic candidate because it is embeddable, portable, widely understood, and supports local reuse through host-controlled `require`.

Profile is now interpreted as a reusable manifest-like recipe template: close enough to Manifest to be understandable, but stripped of runtime binding and authority-bearing fields such as Pod identity, concrete delegated scope, resolved paths, secret material, sockets, runtime state, and active session pointers. The resolver combines Profile with runtime inputs and validation to produce the concrete Manifest snapshot.

This ticket should start with an implementation Pod producing a concrete plan. If the plan respects the boundary below, implementation may proceed in the same worktree.

## Requirements

- Add Profile authoring support without using Nix as the primary profile layer.
- Prefer Lua as the authoring surface unless implementation planning reveals a blocker.
- Provide host-controlled import/module loading rather than relying on installed resource paths:
  - host-provided modules such as `require("insomnia")` / `require("insomnia.profile")` / `require("insomnia.models")`;
  - profile-local reusable modules via controlled local `require`;
  - deny or omit unsafe standard libraries such as `os`, `io`, `debug`, and unrestricted `package`.
- Provide a central public Profile constructor/boundary, likely `profile` / `insomnia.profile`, not a public `mkManifest` as the normal authoring API.
- The value returned by a profile file may be collection/table-like and may map closely to a Profile structure, but it is Profile, not complete Manifest.
- Reject or clearly diagnose Manifest-shaped returns that include runtime-only fields such as `pod.name` or concrete authority-bearing `scope.allow`.
- Keep `--manifest` as the explicit low-level complete Manifest escape hatch.
- Preserve existing profile selection semantics where possible: builtin/default selection, source-qualified selectors, registry/file context, and persisted resolved Manifest snapshots.
- Builtin/default startup must not depend on an external evaluator such as `nix`.
- Support model/context-derived compaction through helpers/policies. True Nix-style recursive sets are not required; use Lua locals and helper APIs instead.
- Scope in Profile may express intent/policy, but concrete scope authority remains runtime/delegation controlled.
- Do not change pod-store/session-log authority boundaries.

## Non-goals

- Do not preserve Nix profile authoring as the main path in this ticket.
- Do not implement a Nix-like custom evaluator.
- Do not expose arbitrary complete Manifest construction as the normal profile API.
- Do not add compatibility layers solely to preserve the abandoned semantic JSON projection.
- Do not broaden SpawnPod scope/profile authority.

## Preflight classification

implementation-ready after implementation Pod plan review.

The high-level product direction is now synchronized in `profile-authoring-requirements-sync`: Profile is a reusable manifest-like recipe template, and Lua/controlled `require` is the current candidate. The implementation Pod must still produce a concrete plan covering crate placement, dependency choice, sandboxing, return contract, resolver integration, builtin/default migration, and tests before coding.

## Acceptance criteria

- An implementation plan is recorded in the ticket thread before code changes are accepted.
- The plan explicitly covers dependency choice, sandbox model, module loading, return contract, Profile-to-Manifest resolver integration, and migration/removal of Nix-primary builtin profile usage.
- A child worktree implementation adds reusable Profile authoring support according to the approved plan.
- Builtin/default profile can resolve without external `nix`.
- Lua profile examples can use host-provided `require("insomnia.*")` modules and local reusable modules.
- Unsafe or unrestricted Lua facilities are unavailable or denied by default.
- Runtime-only Manifest fields in returned Profile values produce clear diagnostics.
- Focused tests cover builtin/default resolution, module loading, local reuse, sandbox denial, invalid Manifest-shaped returns, and Profile-to-Manifest conversion.
- Existing relevant manifest/profile selection tests continue to pass or are updated with intentional behavior changes.


---
