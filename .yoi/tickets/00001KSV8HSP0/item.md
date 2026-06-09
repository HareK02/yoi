---
title: "Sync profile authoring requirements before choosing the language"
state: "closed"
created_at: "2026-05-30T01:39:04Z"
updated_at: "2026-06-01T07:02:44Z"
---

## Background

The `semantic-nix-profiles` implementation direction exposed a deeper product/design issue: choosing a profile authoring language before agreeing on the profile boundary risks recreating the same problem in another syntax. The current Nix-shaped implementation drifted toward manifest-shaped authoring, while the desired authoring experience is portable, reusable, and abstracted through system-provided APIs/functions/modules.

Recent discussion rejected the earlier assumption that a separate semantic JSON projection must be the central abstraction. If that projection preserves almost the same information as Manifest, it may be a failed abstraction. A Nix-like custom subset is also risky because users must learn deviations from real Nix and trust a new evaluator. Snix is technically relevant but currently carries licensing/distribution concerns for direct embedding. Lua is under consideration as a pragmatic embeddable language with long-standing implementation experience, but the profile specification is intentionally not fully decided yet.

The current working interpretation is that Profile is not necessarily a radically higher-level object than Manifest. Instead, Profile may be a reusable, manifest-like recipe template: close enough to Manifest to be understandable, but stripped of Pod identity, environment-specific resolution, and capability-authority fields. The key boundary is not information reduction for its own sake; it is separating reusable recipe content from runtime binding.

This ticket is only for synchronizing requirements and open questions before committing to the exact language/API. It should not implement the profile language.

## Current shared requirements

- Manifest is the complete Pod runtime recipe and remains resolver output / persisted snapshot authority.
- Profile is a reusable manifest-like recipe template, not necessarily a separate semantic-only projection.
- Profile authoring must not become a complete low-level Manifest DSL in another syntax: runtime-only and environment-bound fields must stay out of reusable profiles.
- `--manifest` remains the explicit low-level concrete manifest escape hatch.
- Profile identity is minimal: slug/name and description may be the only direct profile metadata, and may also be supplied by registry/file context where appropriate.
- Instance-specific runtime values such as `pod.name` do not belong in reusable profiles; Pod names come from CLI/TUI/SpawnPod/runtime inputs.
- Scope authority must not move into profiles. `SpawnPod.scope` / explicit delegation remains authoritative for capability expansion.
- Profile may express scope intent/policy, such as workspace read/write, but resolver/runtime must check it against launch workspace and delegated permissions before producing concrete `scope.allow`.
- Environment-specific or resolved values such as absolute paths, runtime directories, sockets, active/pending state, and secret material do not belong in profiles.
- Model, worker/reasoning, context, compaction, memory, web, tool behavior, and related policy may exist in Profile as reusable recipe fields.
- Model/context-derived numeric settings, especially compaction thresholds, should preferably be expressible through helpers/policies such as ratios derived from model metadata, instead of forcing copied raw constants into user profiles.
- User-authored profiles should support practical reuse/composition of partial definitions.
- System-provided APIs should be injected or loaded through a stable mechanism, not by asking users to import files from Insomnia's installed resource layout.
- If Lua is chosen, controlled `require` is a likely core primitive:
  - `require("insomnia")` / `require("insomnia.*")` for host-provided virtual modules;
  - local/profile-directory modules for user reuse;
  - no dependency on installed resource paths.
- If Lua is chosen, the central public constructor should likely be `profile` or `insomnia.profile`, not `mkManifest`. A lower-level `mkManifest` may exist internally or as an advanced escape hatch only if explicitly justified.
- A Lua profile file may return a table/collection that maps closely to a Profile structure. That structure may resemble Manifest minus runtime binding fields.
- A returned table must not be blindly accepted as complete Manifest config. Manifest-shaped returns containing runtime-only fields such as `pod.name` should be diagnosed or routed to `--manifest` instead.
- Nix-style recursive sets are desirable for some authoring patterns, but Lua cannot provide true lazy recursive attrsets. The likely substitute is local bindings plus helper APIs, e.g. load a model first, then derive compaction from its context window or use resolver-side ratio helpers.
- Built-in/default startup should not silently depend on a missing/slow external evaluator unless that dependency is deliberately accepted and documented.

## Current working Lua sketch, not final specification

```lua
local profile = require("insomnia.profile")
local models = require("insomnia.models")
local compact = require("insomnia.compact")
local scope = require("insomnia.scope")

local model = models.catalog("codex-oauth/gpt-5.5")

return profile {
  slug = "default",
  description = "Default coding profile",

  model = model,
  worker = {
    reasoning = "high",
  },
  compaction = compact.ratio {
    threshold = 0.8,
    request = 0.9,
    worker = 0.36,
  },
  scope = scope.workspace_write(),
}
```

This sketch intentionally treats the returned value as Profile, not Manifest. The resolver combines it with runtime Pod name, workspace, delegated scope, path resolution, model catalog data, defaults, and validation to produce a concrete Manifest.

## Open specification questions

- Which authoring language should be supported first: Lua, external Nix wrapper, Starlark/Jsonnet/Nickel/Rhai, or another option?
- If Lua is chosen, which Rust embedding crate/features are acceptable, and how should sandboxing be configured?
- What is the exact module loading model?
  - host-provided `require("insomnia")` / `require("insomnia.*")`;
  - profile-local reusable modules;
  - denied standard libraries such as `os`, `io`, `debug`, `package`;
  - module cache scope and path traversal behavior.
- What exact return contract should profile files use?
  - `return require("insomnia.profile") { ... }`;
  - `return { slug = ..., description = ..., model = ..., compaction = ... }`;
  - explicit profile constructor vs plain table;
  - how invalid Manifest-shaped returns are diagnosed.
- What is the minimal stable Insomnia profile API surface?
  - profile constructors;
  - model catalog access;
  - context/compaction helpers;
  - scope-intent helpers;
  - memory/web/tool policy helpers;
  - extension/merge helpers;
  - optional presets.
- How much direct manifest-like field access should Profile expose versus requiring helper constructors?
- How are profile metadata and registry metadata reconciled when both exist?
- How should existing Nix profile support be handled: keep as advanced/external evaluator path, deprecate, or replace?
- What compatibility/migration story is acceptable for current built-in Nix profile files and existing user profiles?
- What validation and diagnostics are required before any implementation is merged?

## Acceptance criteria

- Capture the agreed requirements above in the work item and keep them synchronized with the design discussion.
- Record the unresolved language/API/sandbox/return-contract questions without prematurely deciding the full specification.
- Ensure `semantic-nix-profiles` implementation work does not proceed as if semantic JSON projection were the accepted design.
- Once the specification is decided, either update this ticket into the implementation ticket or create a follow-up implementation ticket with a clear intent packet.
