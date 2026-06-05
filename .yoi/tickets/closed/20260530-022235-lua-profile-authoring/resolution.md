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
