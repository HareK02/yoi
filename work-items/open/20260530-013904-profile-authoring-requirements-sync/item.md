---
id: 20260530-013904-profile-authoring-requirements-sync
slug: profile-authoring-requirements-sync
title: Sync profile authoring requirements before choosing the language
status: open
kind: task
priority: P2
labels: [manifest, profiles, architecture]
created_at: 2026-05-30T01:39:04Z
updated_at: 2026-05-30T01:39:04Z
assignee: null
legacy_ticket: null
---

## Background

The `semantic-nix-profiles` implementation direction exposed a deeper product/design issue: choosing a profile authoring language before agreeing on the profile boundary risks recreating the same problem in another syntax. The current Nix-shaped implementation drifted toward manifest-shaped authoring, while the desired authoring experience is portable, reusable, and abstracted through system-provided APIs/functions/modules.

Recent discussion ruled out treating the current semantic JSON projection as sufficient. If `JSON Profile -> Manifest` preserves almost the same information as Manifest, it is an abstraction failure. Likewise, a Nix-like custom subset is risky because users must learn deviations from real Nix and trust a new evaluator. Snix is technically relevant but currently carries licensing/distribution concerns for direct embedding. Lua is under consideration as a pragmatic embeddable language with long-standing implementation experience, but the profile specification is intentionally not decided yet.

This ticket is only for synchronizing requirements and open questions before committing to a language/API. It should not implement the profile language.

## Current shared requirements

- Profile authoring must not be a low-level Manifest DSL in another syntax.
- The runtime Manifest remains resolver output, not the normal profile authoring surface.
- `--manifest` remains the explicit low-level concrete manifest escape hatch.
- Profile identity is minimal: slug/name and description may be the only direct profile metadata, and may also be supplied by registry/file context where appropriate.
- Instance-specific runtime values such as `pod.name` do not belong in reusable profiles; Pod names come from CLI/TUI/SpawnPod/runtime inputs.
- Scope authority must not move into profiles. `SpawnPod.scope` / explicit delegation remains authoritative for capability expansion.
- Reasoning, model, context, compaction, memory, web, and tool behavior should be expressed through higher-level profile APIs/policies/presets and manifestized by Rust.
- Model/context-derived numeric settings, especially compaction thresholds, should be derived from model metadata/policy in one place rather than copied as raw constants into user profiles.
- Built-in/default startup should not silently depend on a missing/slow external evaluator unless that dependency is deliberately accepted and documented.
- User-authored profiles should support practical reuse/composition of partial definitions.
- System-provided APIs should be injected or loaded through a stable mechanism, not by asking users to import files from Insomnia's installed resource layout.
- If Lua is chosen, `require("insomnia")` / `require("insomnia.*")` as host-provided virtual modules and controlled local `require` are candidate mechanics, but not yet final.
- A returned plain table may be acceptable as a profile artifact, but arbitrary tables must not be blindly interpreted as Manifest-shaped config. The acceptance boundary is still open.

## Open specification questions

- Which authoring language should be supported first: Lua, external Nix wrapper, Starlark/Jsonnet/Nickel/Rhai, or another option?
- If Lua is chosen, which Rust embedding crate/features are acceptable, and how should sandboxing be configured?
- What is the exact module loading model?
  - host-provided `require("insomnia")` / `require("insomnia.*")`;
  - profile-local reusable modules;
  - denied standard libraries such as `os`, `io`, `debug`, `package`;
  - module cache scope and path traversal behavior.
- What exact return contract should profile files use?
  - `return require("insomnia.profile").coder { ... }`;
  - `return { slug = ..., description = ..., policy = ... }`;
  - explicit profile constructor vs plain table;
  - how invalid Manifest-shaped returns are diagnosed.
- What is the minimal stable Insomnia profile API surface?
  - profile constructors;
  - presets;
  - model references;
  - context/compaction policies;
  - memory/web/tool policy helpers;
  - extension/merge helpers.
- How are profile metadata and registry metadata reconciled when both exist?
- How should existing Nix profile support be handled: keep as advanced/external evaluator path, deprecate, or replace?
- What compatibility/migration story is acceptable for current built-in Nix profile files and existing user profiles?
- What validation and diagnostics are required before any implementation is merged?

## Acceptance criteria

- Capture the agreed requirements above in the work item and keep them synchronized with the design discussion.
- Record the unresolved language/API/sandbox/return-contract questions without prematurely deciding the specification.
- Ensure `semantic-nix-profiles` implementation work does not proceed as if semantic JSON projection were the accepted design.
- Once the specification is decided, either update this ticket into the implementation ticket or create a follow-up implementation ticket with a clear intent packet.
