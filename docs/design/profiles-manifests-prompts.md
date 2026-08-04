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

Yoi Profiles are data artifacts resolved from builtin profile definitions, `profiles.toml`, Decodal source archives, or explicit JSON/TOML artifacts. Decodal is the authoring syntax used by the Workspace profile editor and Backend Runtime archive path; JSON/TOML artifacts are the low-level resolver interchange format. Runtime launch should not depend on executable profile scripts.

## Manifests

A resolved Manifest is the concrete contract used to create or restore a Worker. It carries defaults, resolved paths, permissions, scope, prompt references, provider/model decisions, and runtime identity.

Source/partial layers may omit fields. Resolved manifests should be explicit enough that Worker creation does not depend on ambient configuration later changing under it.

`--manifest <path>` exists as an explicit low-level escape hatch. Normal fresh startup should select a Profile through `profiles.toml` / builtin defaults rather than ambient manifest cascades.

For normal Profile/default startup, a workspace may add `.yoi/override.local.toml` as a final local manifest layer. Yoi discovers the nearest ancestor `.yoi/override.local.toml` from the workspace base used for profile resolution, resolves relative paths in that file against its containing `.yoi` directory, and applies it after the selected Profile and builtin defaults. This file is intended for machine-local choices such as provider/model, worker language, prompt pack, and permission policy tweaks; it is ignored by git via the repository `*.local.*` rule. It is not applied in explicit `--manifest <path>` mode, and it cannot set `worker.name` because Worker identity remains a runtime input.

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

Builtin resources should be embedded at compile time. User/project profiles, explicit profile paths, prompt overlays, provider/model overrides, and explicit manifests remain filesystem-based.

## Why this separation matters

Without this split, configuration becomes unreproducible: a Profile might accidentally depend on a parent Worker's socket, a prompt override might act like hidden state, or a restored Worker might observe different defaults than the run that created it.

The boundaries make it clear which information is reusable authoring, which is resolved runtime contract, and which is durable run history.
