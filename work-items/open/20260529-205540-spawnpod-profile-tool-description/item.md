---
id: 20260529-205540-spawnpod-profile-tool-description
slug: spawnpod-profile-tool-description
title: SpawnPod profile selection and templated tool description
status: open
kind: feature
priority: P2
labels: [pod, manifest, tools, workflow]
created_at: 2026-05-29T20:55:40Z
updated_at: 2026-05-29T20:55:40Z
assignee: null
legacy_ticket: null
---

## Background

`SpawnPod` is becoming the main mechanism for hierarchical orchestration. The workflow model now distinguishes role-specific child Pods: lower orchestrators, coder Pods, and external reviewer Pods. These roles should be selectable by profile, but the current `SpawnPod` tool has no profile field and no LLM-visible route to discover profile selectors.

A separate `ListProfiles` tool would expose mostly static or semi-static affordance information as another model action. The desired design is instead to make profile choices part of the `SpawnPod` tool's own guidance: render the available profile selectors into the tool description, and repeat the same selector list in invalid/ambiguous/no-default diagnostics.

Current implementation notes:

- Tool metadata is defined by `llm_worker::tool::ToolMeta`; `ToolDefinition` factories return `(ToolMeta, Arc<dyn Tool>)` and `ToolServerHandle::flush_pending()` materializes them before request construction.
- Tool descriptions are currently hard-coded strings/doc comments in each tool factory. `crates/pod/src/spawn/tool.rs` has a static `DESCRIPTION` and `SpawnPodInput` only contains `name`, `instruction`, `task`, and `scope`.
- `SpawnPod` currently builds an internal `PodManifestConfig` directly from the parent model, optional instruction override, and delegated scope, then launches the child with hidden `--spawn-config-json`. It does not use profile resolution.
- Prompt assets are centralized under `resources/prompts`; Pod-owned prompt injection strings are represented by `PodPrompt` and rendered by `PromptCatalog` through minijinja. `resources/prompts/internal.toml` has build-time coverage against `PodPrompt` variants.
- Profile discovery/resolution already exists in `manifest`: `ProfileDiscovery::for_cwd`, `ProfileRegistry`, `ProfileSelector`, `NixProfileResolver`, and source-qualified selectors such as `project:coder`, `project:reviewer`, and `builtin:default`.

## Requirements

- Add an optional `profile` field to `SpawnPodInput`.
- Make `SpawnPod`'s LLM-facing tool description include the currently discoverable profile selectors and the effective default profile.
- Do not add a separate `ListProfiles` tool for this feature.
- The profile list in the tool description must come from the same builtin/user/project profile discovery rules used by the profile launcher path.
- If profile discovery fails, the tool should still be registered with a clear diagnostic in its description rather than making Pod startup fail solely because the description could not list profiles.
- If `profile` is omitted, `SpawnPod` resolves the effective default profile. With the builtin default profile present, ordinary omission should keep working.
- If `profile` is invalid, ambiguous, unsupported, or omitted while no default can be resolved, the tool error must include a compact available-profile list and source-qualified suggestions.
- `SpawnPod.profile` should initially accept registry/default selectors only: `default`, `builtin:<name>`, `user:<name>`, `project:<name>`, and unqualified names only when unambiguous. Raw path selectors, `path:<path>`, relative paths, absolute paths, and `.nix` path-like values are rejected for the tool path.
- `SpawnPod.scope` remains the only delegated capability. A profile selected through `SpawnPod` must not be able to expand `scope.allow` beyond the explicit tool argument.
- `SpawnPod.name` always overrides `pod.name` in the resolved child manifest.
- `SpawnPod.instruction`, if present, is a typed override for the resolved profile's `worker.instruction` only; it must not replace the whole profile.
- Profile-selected spawn should preserve profile-owned role configuration such as model, worker settings, tools/memory/web policy, and prompt pack where applicable, subject to the explicit `name` and `scope` overrides.
- Existing hidden `--spawn-config-json` remains the internal launch handoff. `SpawnPod` resolves/merges profile data in the parent process and passes the resulting manifest config/snapshot to the child; it should not simply exec `insomnia-pod --profile`.
- Documentation/workflows should show `SpawnPod(profile = "project:coder")`, `SpawnPod(profile = "project:reviewer")`, and optionally `project:orchestrator` as role selectors, while keeping scope authority separate from profile role selection.

## Tool description templating direction

Use the existing prompt infrastructure rather than scattering another large hard-coded string.

Acceptable implementation shape:

- Add a Pod-owned prompt/catalog entry for the `SpawnPod` tool description, e.g. `spawn_pod_tool_description`, with minijinja variables such as `available_profiles`, `default_profile`, and `profile_diagnostic`.
- Render this prompt when registering `SpawnPod` in `register_pod_tools`, using the Pod cwd as the profile discovery base.
- Keep the rendered description as `ToolMeta.description`; the tool metadata still remains session-scoped after registration.
- The same formatter used for the description should be reusable by error diagnostics so invalid profile errors repeat the available selectors.

If a more general tool-description catalog is introduced, keep the initial scope narrow: it must support `SpawnPod` without forcing every built-in tool to migrate in the same ticket.

## Acceptance criteria

- `SpawnPod` schema exposes optional `profile` with clear field docs.
- `SpawnPod` tool description includes a compact available-profile block and default-profile guidance.
- `SpawnPod` invalid/ambiguous/no-default profile errors include the same compact selector list and tell the model to use a listed selector.
- `SpawnPod(profile = "project:reviewer")` resolves the project reviewer profile and applies its role config while replacing `pod.name` and `scope.allow` with the explicit `SpawnPod` values.
- `SpawnPod` with omitted profile resolves the effective default profile.
- `SpawnPod(profile = "./reviewer.nix")`, `SpawnPod(profile = "path:./reviewer.nix")`, and absolute profile paths are rejected with an explanation that the tool accepts registry/default selectors only.
- Ambiguous unqualified profile names fail closed with source-qualified suggestions.
- A profile's `scope.allow` cannot grant access not present in `SpawnPod.scope`.
- Existing `SpawnPod` behavior that matters for lifecycle remains intact: registry reservation/rollback, scope revocation from the spawner, callback wiring, child socket wait, and initial `Method::Run` confirmation.
- Unit/integration tests cover description rendering, selector formatting, omitted default resolution, invalid selector diagnostics, profile config application, explicit instruction override, and scope authority replacement.
- Documentation and `.insomnia/workflow/` references explain profile-based coder/reviewer/orchestrator spawning without introducing a `ListProfiles` tool.

## Non-goals

- Do not introduce an LLM-callable `ListProfiles` tool.
- Do not enable arbitrary profile path evaluation through `SpawnPod`.
- Do not revive manifest cascade or generic overlay layers.
- Do not redesign prompt-loader source selection for `$user` / `$workspace` profile prompt refs in this ticket unless it is required to keep current behavior correct.
- Do not implement encrypted secret storage; profiles may still contain unresolved typed secret refs as currently documented.
