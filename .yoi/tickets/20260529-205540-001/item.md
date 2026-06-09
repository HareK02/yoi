---
title: "SpawnPod profile selection and templated tool description"
state: "closed"
created_at: "2026-05-29T20:55:40Z"
updated_at: "2026-05-30T05:19:46Z"
---

## Background

`SpawnPod` is becoming the main mechanism for hierarchical orchestration. The workflow model now distinguishes role-specific child Pods: lower orchestrators, coder Pods, and external reviewer Pods. These roles should be selectable by profile, but the current `SpawnPod` tool has no profile field and no LLM-visible route to discover profile selectors.

A separate `ListProfiles` tool would expose mostly static or semi-static affordance information as another model action. The desired design is instead to make profile choices part of the `SpawnPod` tool's own guidance: render the available profile selectors into the tool description, and repeat the same selector list in invalid/ambiguous/no-default diagnostics.

Current implementation notes:

- Tool metadata is defined by `llm_worker::tool::ToolMeta`; `ToolDefinition` factories return `(ToolMeta, Arc<dyn Tool>)` and `ToolServerHandle::flush_pending()` materializes them before request construction.
- Tool descriptions are currently hard-coded strings/doc comments in each tool factory. `crates/pod/src/spawn/tool.rs` has a static `DESCRIPTION` and `SpawnPodInput` only contains `name`, `instruction`, `task`, and `scope`.
- `SpawnPod` currently builds an internal `PodManifestConfig` directly from selected pieces of the parent resolved manifest plus tool input, then launches the child with hidden `--spawn-config-json`. It copies the parent model and session trace flag, applies optional instruction override, and uses the delegated scope from the tool call. It does not use profile resolution and it does not copy the parent resolved manifest wholesale.
- Prompt assets are centralized under `resources/prompts`; Pod-owned prompt injection strings are represented by `PodPrompt` and rendered by `PromptCatalog` through minijinja. `resources/prompts/internal.toml` has build-time coverage against `PodPrompt` variants.
- Profile discovery/resolution already exists in `manifest`, though the concrete profile authoring layer is being revised away from Nix-primary semantics. SpawnPod profile selection should use the same effective profile registry/default semantics as the normal launcher path once that resolver is available.

## Requirements

- Add an optional `profile` field to `SpawnPodInput`.
- `SpawnPod.profile` accepts three conceptual selector classes:
  - omitted or `"default"`: resolve the effective child default profile;
  - `"inherit"`: derive child config from the spawner's resolved Manifest, extracting only reusable profile-like fields;
  - `<slug>` / source-qualified selectors such as `builtin:<slug>`, `user:<slug>`, `project:<slug>`: resolve a discovered role profile.
- `inherit` is distinct from reusing the profile source that created the parent. It means extracting reusable configuration from the parent resolved Manifest. Re-evaluating or reusing the parent's original Profile source is a separate concept and is not required here.
- Make `SpawnPod`'s LLM-facing tool description include the currently discoverable profile selectors, the effective default profile, and the special `inherit` selector.
- Do not add a separate `ListProfiles` tool for this feature.
- The profile list in the tool description must come from the same builtin/user/project profile discovery rules used by the profile launcher path.
- If profile discovery fails, the tool should still be registered with a clear diagnostic in its description rather than making Pod startup fail solely because the description could not list profiles.
- If `profile` is omitted, `SpawnPod` resolves the effective default profile. With the builtin default profile present, ordinary omission should keep working.
- If `profile` is invalid, ambiguous, unsupported, or omitted while no default can be resolved, the tool error must include a compact available-profile list, source-qualified suggestions, and mention `inherit` where appropriate.
- `SpawnPod.profile` should initially accept registry/default selectors and `inherit` only: `default`, `inherit`, `builtin:<name>`, `user:<name>`, `project:<name>`, and unqualified names only when unambiguous. Raw path selectors, `path:<path>`, relative paths, absolute paths, and `.nix`/`.lua` path-like values are rejected for the tool path.
- `SpawnPod.scope` remains the only delegated capability. A profile selected through `SpawnPod`, including `inherit`, must not be able to expand `scope.allow` beyond the explicit tool argument.
- `SpawnPod.name` always overrides any resolved/derived `pod.name` in the child manifest.
- `SpawnPod.instruction`, if present, is a typed override for the selected profile's or inherited config's `worker.instruction` only; it must not replace the whole profile/config.
- Profile-selected spawn should preserve profile-owned role configuration such as model, worker settings, tools/memory/web policy, and prompt pack where applicable, subject to the explicit `name` and `scope` overrides.
- `inherit` should preserve reusable parent-owned configuration such as model, worker settings, compaction, tools/memory/web policy, prompt pack, and session diagnostics where applicable, subject to the explicit `name`, `scope`, and optional `instruction` overrides.
- `inherit` must not preserve runtime-bound or authority-bearing parent fields: parent `pod.name`, concrete parent `scope.allow`/`scope.deny`, resolved runtime paths, sockets, session/pod-store state, spawned-child registry state, callback addresses, or raw resolved secret material.
- Existing hidden `--spawn-config-json` remains the internal launch handoff. `SpawnPod` resolves/merges selected profile data in the parent process and passes the resulting manifest config/snapshot to the child; it should not simply exec `insomnia-pod --profile`.
- Documentation/workflows should show `SpawnPod(profile = "project:coder")`, `SpawnPod(profile = "project:reviewer")`, optionally `project:orchestrator`, and `SpawnPod(profile = "inherit")` as explicit choices while keeping scope authority separate from profile role selection.

## Tool description templating direction

Use the existing prompt infrastructure rather than scattering another large hard-coded string.

Acceptable implementation shape:

- Add a Pod-owned prompt/catalog entry for the `SpawnPod` tool description, e.g. `spawn_pod_tool_description`, with minijinja variables such as `available_profiles`, `default_profile`, `special_selectors`, and `profile_diagnostic`.
- Render this prompt when registering `SpawnPod` in `register_pod_tools`, using the Pod cwd as the profile discovery base.
- Keep the rendered description as `ToolMeta.description`; the tool metadata still remains session-scoped after registration.
- The same formatter used for the description should be reusable by error diagnostics so invalid profile errors repeat the available selectors.

If a more general tool-description catalog is introduced, keep the initial scope narrow: it must support `SpawnPod` without forcing every built-in tool to migrate in the same ticket.

## Acceptance criteria

- `SpawnPod` schema exposes optional `profile` with clear field docs for `default`, `inherit`, and profile slug/source-qualified selectors.
- `SpawnPod` tool description includes a compact available-profile block, default-profile guidance, and the `inherit` special selector.
- `SpawnPod` invalid/ambiguous/no-default profile errors include the same compact selector list and tell the model to use a listed selector or `inherit`.
- `SpawnPod(profile = "project:reviewer")` resolves the project reviewer profile and applies its role config while replacing `pod.name` and `scope.allow` with the explicit `SpawnPod` values.
- `SpawnPod` with omitted profile resolves the effective default profile.
- `SpawnPod(profile = "inherit")` derives child config from the parent resolved Manifest's reusable fields while replacing `pod.name`, `scope.allow`, and optional `worker.instruction` with the explicit SpawnPod values.
- `SpawnPod(profile = "./reviewer.lua")`, `SpawnPod(profile = "path:./reviewer.lua")`, legacy `.nix` path-like selectors, and absolute profile paths are rejected with an explanation that the tool accepts `default`, `inherit`, or registry selectors only.
- Ambiguous unqualified profile names fail closed with source-qualified suggestions.
- A profile's or inherited config's `scope.allow` cannot grant access not present in `SpawnPod.scope`.
- Existing `SpawnPod` behavior that matters for lifecycle remains intact: registry reservation/rollback, scope revocation from the spawner, callback wiring, child socket wait, and initial `Method::Run` confirmation.
- Unit/integration tests cover description rendering, selector formatting, omitted default resolution, `inherit`, invalid selector diagnostics, profile config application, explicit instruction override, and scope authority replacement.
- Documentation and `.insomnia/workflow/` references explain profile-based coder/reviewer/orchestrator spawning without introducing a `ListProfiles` tool.

## Non-goals

- Do not introduce an LLM-callable `ListProfiles` tool.
- Do not enable arbitrary profile path evaluation through `SpawnPod`.
- Do not confuse `inherit` with reusing the parent's original Profile source. `inherit` is derived from the parent resolved Manifest; parent Profile source reuse can be a later explicit feature if needed.
- Do not revive manifest cascade or generic overlay layers.
- Do not redesign prompt-loader source selection for `$user` / `$workspace` profile prompt refs in this ticket unless it is required to keep current behavior correct.
- Do not implement encrypted secret storage; profiles may still contain unresolved typed secret refs as currently documented.
