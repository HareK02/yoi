# Profiles, Manifests, and prompts

Profiles are reusable recipes. Resolved Manifests are runtime contracts. Prompt resources are managed assets. Keeping those layers separate prevents runtime state from leaking into reusable configuration.

## Profiles

A Profile describes how a Worker should normally be built: worker language, model/provider selectors, prompt choices, tool policy defaults, and other reusable preferences.

A Profile should not contain runtime-bound fields:

- `worker.name`
- concrete delegated `scope.allow`
- sockets or process identifiers
- session pointers
- restored spawned-child state
- raw secret values

Those fields depend on one run, one parent, or one machine. Putting them in a reusable Profile makes reuse unsafe.

Yoi Profiles are data artifacts resolved from builtin profile definitions, `profiles.toml`, Workspace Config values, or explicit JSON/TOML artifacts. Decodal is the Workspace authoring syntax; JSON/TOML artifacts are the low-level resolver interchange format. Runtime launch does not depend on executable profile scripts.

### Workspace registration and migration

Register the Profile **value**, not a source pathname:

```dcdl
{
    profile = {
        default_profile = "project:my-companion";
        entries = [{
            selector = "project:my-companion";
            label = "My Companion";
            description = "My reusable Worker recipe";
            profile = import "./profiles/my-companion.dcdl";
        }];
    };
} as WorkspaceConfigSchema
```

The imported file can contain `{ worker = { mode = "wip"; }; model = { ref = "codex-oauth/gpt-5.6-sol"; }; feature = { task = { enabled = true; }; }; }`. That record can also be written directly at `entries[0].profile`. Composition uses ordinary Decodal expressions, for example `(import "./profiles/base.dcdl") // { worker = { mode = "wip"; }; }` for an explicit patch, or `&` for compatible refinement. No dedicated Profile file or `profiles/` directory is required. Imports are relative to the **containing virtual config file**, not a host filesystem path; missing imports and virtual-tree escapes are rejected.

`selector` must be a nonempty `project:*` selector, unique in the Workspace. `label` and `description` are optional display metadata. An omitted `default_profile` selects `builtin:companion`; an explicitly unknown default is an error, not a fallback. A project Profile is a partial recipe resolved against `WorkerManifestConfig::resolution_defaults()`, **not** implicitly inherited from a builtin role. Explicit values remain explicit and omitted Profile fields remain absent in the transported value. To inherit a builtin recipe, import its public value explicitly; there is no implicit builtin inheritance.

### Public builtin imports

Workspace Decodal can import embedded Profile values through the read-only `$builtin/` namespace:

```dcdl
(import "$builtin/profiles/companion.dcdl") // {
    worker = { mode = "wip"; };
    feature = { task = { enabled = true; }; ticket = { enabled = false; }; };
}
```

Use this expression anywhere a Profile value is accepted, including `profile.entries[*].profile` or an imported Workspace recipe. `$builtin/profiles/companion.dcdl` is a source path, not the launch selector `builtin:companion`. Public paths under `$builtin/profiles/` are `base.dcdl`, `default.dcdl`, `standalone.dcdl`, `standalone-subjektiv.dcdl`, `standalone-subjektiv-consolidation.dcdl`, `coder.dcdl`, `companion.dcdl`, `intake.dcdl`, `reviewer.dcdl`, `orchestrator.dcdl`, `job.dcdl`, `backend-job.dcdl`, `memory-consolidation.dcdl`, and `subjektiv-memory-consolidation.dcdl`. This is an explicit embedded catalog, not arbitrary resource or filesystem access. Unknown paths, namespace escapes, host paths and network URLs are errors.

Relative imports **inside a builtin source** resolve against its builtin virtual directory: companion's `./base.dcdl` means `$builtin/profiles/base.dcdl`, never the Workspace's `profiles/base.dcdl`. Relative imports in Workspace files continue to use their containing Workspace virtual directory. Builtin values obey ordinary Decodal composition: `//` applies an explicit patch and `&` refines compatible values (conflicting explicit values remain errors). Importing a builtin does not add a special merge or fallback mechanism.

Builtin dependencies share the Workspace snapshot budgets: at most 256 entries across Workspace entries and distinct observed builtin sources, 256 KiB per source, and 4 MiB of total source bytes. Unused Workspace entries still count; repeated imports of the same builtin count once. The observed import graph must be acyclic and at most 32 edges deep, **including the synthetic evaluation wrapper's import of the entrypoint**. These guards apply before the loader returns sources to Decodal, including imports in lazy object fields; builtins do not bypass them. Launch archives retain their separate transport limits (64 sources, 256 KiB per source, 1 MiB total source bytes).

The former `entries = [{ selector = "project:name"; source = "profiles/name.dcdl"; }]` registration is no longer accepted. Migrate it explicitly to `profile = import "./profiles/name.dcdl";`, preserving the selector, label, description and default. For registrations in a nested config file, adjust the relative import to the original virtual-tree entry (for example `../profiles/name.dcdl`). Do not rename or recreate the Profile file merely for registration. Previously saved source-based revisions remain readable as config data and are not rewritten, deleted, or silently substituted with a builtin. Their Profile projection/launch reports `profile_source_registration_removed` with the replacement form; saves under the new schema reject the old field. Existing Workers continue using their saved Manifest while the author migrates and saves the config.

The schema contribution separates its materialized registration schema from an optional, fingerprint-bound `authoring_source`. The latter supplies editor field suggestions inside array elements and partial Profile records, plus a separate constraint-only analysis pass for supplied nested values (including imported recipes); that pass never materializes the shape, makes omitted fields required, or supplies defaults to the saved projection. Editor diagnostics point to the supplied value's virtual file/span. The authoring shape is never substituted for the persisted evaluation schema. Decodal structural diagnostics cover registration and nested Profile field types; the shared Backend save boundary additionally checks full Profile/Manifest semantics (including mode values, forbidden runtime fields, duplicate selectors and default references). UI and WIP saves use that same boundary; invalid candidates leave the active config revision untouched.

`scope` and `delegation_scope` retain both supported forms: a string such as `"workspace_read"` / `"workspace_write"`, or a table such as `{ intent = "workspace_read"; deny_write = ["private"]; symlink_policy = "resolved"; }`. Decodal 0.4 has no union range, so these fields are opaque to the editor constraint pass; a non-materialized table hint supplies nested completion without rejecting the string form or rewriting it. The shared Backend save validator checks scope forms, intent values and table fields (including their types and unknown keys). Other structurally typed fields such as `worker.mode` and `feature.task.enabled` retain editor type diagnostics. Completion hints never become Profile defaults or saved values.

Extraction `reasoning` in both `feature.memory` and `feature.subjektiv`, like `engine.reasoning`, accepts either a string effort or a signed integer budget. These union values are also opaque to editor constraints and semantically checked on save. Compaction authoring covers the complete token settings and their supported `compact_*` aliases, plus the ratio helper's `request` / `worker` aliases; values are not renamed or defaulted during Workspace evaluation. The shared test corpus `resources/config-schema/profile-authoring-values.json` proves exact values in native/WASM analysis and effective settings through save/projection/Runtime consumption. Its canonical compaction case must cover every serialized `CompactionConfigPartial` field, making future contract drift visible in tests.

## Manifests

A resolved Manifest is the concrete contract used to create or restore a Worker. It carries defaults, resolved paths, permissions, scope, prompt references, provider/model decisions, and runtime identity.

Source/partial layers may omit fields. Resolved manifests should be explicit enough that Worker creation does not depend on ambient configuration later changing under it.

`--manifest <path>` exists as an explicit low-level escape hatch. Normal fresh startup selects a `builtin:*` or `project:*` Profile from the Backend-managed Workspace Config revision rather than applying an ambient manifest cascade.

Project Profiles are evaluated as part of the revisioned Virtual Config, including its imports and value composition. The Backend packages the **evaluated selected value** into a digest-bound JSON entry in the existing Profile archive transport, and binds the launch bundle to the Workspace identity, revision, tree/projection digests and schema/toolchain fingerprints. Runtime verifies the archive and resolves that exact value without re-evaluating Decodal, scanning imports or reading Workspace files. When Workspace evaluation imports builtins, the archive additionally seals the exact observed transitive builtin closure for the Workspace contract: canonical `$builtin/profiles/...` source keys, original Decodal bytes, content types, sizes and source digests, all bound by the outer archive and launch bundle digests. These are provenance dependencies, not runtime entrypoints or runtime import instructions. Missing or corrupt provenance fails verification even though the entrypoint is JSON; no local file or current embedded builtin can rescue it. Builtin Profiles selected directly retain their embedded Decodal archive path. Changing an embedded builtin so that it changes the evaluated Workspace value requires an **explicit config save/re-evaluation before a fresh launch**. The Backend compares current evaluation with the saved projection digest and rejects a mismatch rather than silently adopting the new value. T-711 retains the existing version-2 toolchain contract/fingerprint tuple so existing stored contracts remain readable; builtin provenance is bound by the archive and bundle digests, not a new catalog fingerprint field. Neither a builtin update nor a config save rewrites a sealed archive or silently rebases an existing Worker. Saving config changes only future fresh launches; the Worker persists the resulting resolved Manifest and restore uses that saved snapshot rather than selecting the current recipe again. Files below the Workdir are not implicit Profile override layers.

## Local stdio MCP server declarations

Profiles and manifest layers may declare named local stdio MCP servers under `mcp.stdio_server`. This is a typed configuration surface only. Declaring a server does not start a subprocess, discover packages, negotiate MCP capabilities, or register tools/resources/prompts.

Example TOML artifact fragment:

```toml
[[mcp.stdio_server]]
name = "filesystem"
command = "node"
args = ["server.js", "--root", "."]

[mcp.stdio_server.cwd]
kind = "path"
path = "./mcp"

[mcp.stdio_server.env]
inherit = ["PATH"]

[mcp.stdio_server.env.set.SAFE_MODE]
kind = "literal"
value = "1"

[mcp.stdio_server.env.set.API_TOKEN]
kind = "secret_ref"
ref = "providers/mcp-token"

[mcp.stdio_server.env.set.UPSTREAM_TOKEN]
kind = "env_ref"
name = "MCP_UPSTREAM_TOKEN"
```

`command` is a direct executable name/path, not a shell string. `args` are passed as argv entries by future lifecycle code. `cwd.kind = "path"` is resolved relative to the Profile or manifest layer; omit `cwd` or use `{ kind = "inherit" }` when the lifecycle caller should choose. Environment handling is explicit: future spawn code should inherit only names listed in `env.inherit` and set only variables in `env.set`. `literal` values are for non-secret data; credentials should use `secret_ref` or explicit `env_ref`. Diagnostics and Debug output must redact env literal values and must not print secret plaintext.

Local stdio MCP servers are ordinary local executables running with the user's OS permissions. Yoi's feature flags, Plugin permissions, and MCP config validation are not an operating-system sandbox and cannot prevent filesystem/network/process side effects once a later lifecycle implementation chooses to spawn a configured server.

## SubWorkers

`SubWorkerSpawn.profile` is optional and resolves through defaults when omitted. The only concrete capability delegation in the tool call is `SubWorkerSpawn.scope`, and it must be a subset of the parent Worker's effective scope.

`inherit` derives reusable settings from the parent's resolved Manifest while replacing child identity and delegated scope. It should not blindly reuse the parent's original Profile source or runtime state.

## Prompt resources

Prompts live under `resources/prompts` so builtins, project overrides, and user overrides have one asset boundary.

The prompt layer should explain policy and behavior, but it should not smuggle volatile state into model context. Runtime facts that affect later turns must still go through history.

Builtin resources should be embedded at compile time. Project Profiles and prompt overlays belong to the Backend-managed revisioned Workspace Config and travel as digest-bound source archives. User Profile registries, provider/model catalog overrides, and explicit low-level Manifests remain filesystem-based where those local resolution paths are used.

## Why this separation matters

Without this split, configuration becomes unreproducible: a Profile might accidentally depend on a parent Worker's socket, a prompt override might act like hidden state, or a restored Worker might observe different defaults than the run that created it.

The boundaries make it clear which information is reusable authoring, which is resolved runtime contract, and which is durable run history.
