# Plugin packages and discovery

Plugin packages are a distribution format, not an authority boundary. A package can be found on disk, inspected, validated, and cached without registering any Hook, exposing any Tool, starting any process, or initializing any WASM module.

The initial goal is a durable `.yoi-plugin` package format that later Tickets can implement in independent layers: discovery, archive validation/cache materialization, manifest/profile enablement, Plugin permission policy, declarative hooks, WASM runtime support, and any future MCP bridge.

## Package shape

A `.yoi-plugin` file is a single-file archive. The initial archive format should be a constrained ZIP profile because it is easy to inspect without executing code and can carry text manifests, WASM modules, schemas, and license material.

The archive root must contain `plugin.toml` directly at the root. Packages should not require a wrapping directory whose name must match the plugin id.

Recommended root layout:

```text
plugin.toml              # required package manifest
module.wasm              # optional; required when plugin.toml declares a WASM runtime
hooks/*.toml             # optional declarative hook definitions
schemas/*.schema.json    # optional JSON schemas for configuration or tool input/output
README.md                # recommended human description
LICENSE*                 # recommended license text
assets/**                # optional non-executable data assets
```

The package layout is intentionally data-first. Placing a package in a store must never execute `module.wasm`, register hook metadata, or scan assets as prompts. Those steps happen only after explicit enablement and policy resolution.

## `plugin.toml`

`plugin.toml` is the package authority for package identity and declared needs. It is not the authority for runtime grants.

Currently implemented strict `plugin.toml` shape:

```toml
schema_version = 1
id = "example.summarizer"
name = "Example Summarizer"
version = "0.1.0"
description = "Adds a custom summary command."
surfaces = ["hook"]

[[hooks]]
id = "summary"
file = "hooks/summary.md"
```

The package archive must contain both root `plugin.toml` and the referenced `hooks/summary.md` entry. Optional WASM metadata is accepted only for the declared future runtime boundary and is not executed:

```toml
[runtime]
kind = "wasm"
entry = "plugin.wasm"
abi = "yoi-plugin-wasm-1"
```

First-pass fields accepted by the parser:

- `schema_version`: required integer; unsupported versions fail closed.
- `id`: required unqualified local id. It is scoped by the source that discovered the package; it is not globally unique by itself.
- `name`, `version`, `description`: human metadata used in listings and diagnostics.
- `surfaces`: optional declared contribution surface names.
- `runtime`: optional WASM metadata only. Discovery records metadata and never executes it.
- `hooks`: optional hook metadata. Discovery records metadata and does not register hooks.

Future descriptor sections such as `[package]`, `[permissions]`, richer `contributions`, or `runtime.kind = "declarative"` are aspirational and are intentionally rejected by the current strict parser until implemented safely.

The `source` is not read from `plugin.toml`. It is assigned by the store that discovered the package.

## Stores, sources, and trust

Discovery should scan explicit stores and attach a source kind to each package:

- `builtin:<id>`: packages shipped with Yoi or installed as part of the binary distribution.
- `user:<id>`: packages discovered under `${XDG_DATA_HOME:-~/.local/share}/yoi/plugins/`.
- `project:<id>`: packages discovered under `<workspace>/.yoi/plugins/`.

Packages under `${XDG_DATA_HOME:-~/.local/share}/yoi/plugins/` or `<workspace>/.yoi/plugins/` are discovery only. Their presence is never permission to register Hooks or Tools, initialize WASM, start processes, open files, use network providers, read secrets, or launch MCP servers.

Trust differs by source, but none of the sources is self-authorizing:

- Builtin packages can be trusted as shipped code/data, but still require explicit enablement for a Pod/Profile when they affect runtime behavior.
- User packages are local user-installed artifacts and should be visible to workspaces, but they cannot bypass manifest/profile/tool/scope/secret policy.
- Project packages are repository-controlled artifacts and should be treated as untrusted until explicitly enabled by local policy. Cloning a repository must not be enough to execute a package.

## Identity and selector rules

Runtime identity is source-qualified: `builtin:<id>`, `user:<id>`, and `project:<id>` are distinct plugins even when `<id>` is the same string.

Durable enablement records should use source-qualified ids. Ambiguous unqualified ids fail closed. The implementation may offer convenience listing or search by bare id, but any operation that enables a package, grants permission, pins a digest, or records restored runtime state should require the fully qualified id.

Collision handling:

- Two packages with the same source-qualified id in the same effective store set are a discovery diagnostic and neither candidate is enabled implicitly.
- A `user:example` package does not override `builtin:example` unless a future explicit override rule says so.
- A `project:example` package does not override `user:example` or `builtin:example` by name alone.

## Discovery versus enablement

Discovery is a read-only inventory operation. It may report package metadata, validation errors, source, canonical store path, and deterministic digest. It must not initialize any runtime contribution.

Enablement is a resolved runtime plan. It should come from Profile/manifest configuration or another explicit local policy layer, then be recorded into the resolved Manifest/session metadata used to start the Pod. Restored Pods should use that resolved enabled-plugin plan instead of silently re-running fresh discovery and picking newer packages. Fresh discovery must not silently upgrade a restored Pod.

A minimal implemented enablement record is shaped like this. `version` is an exact package-version requirement; richer range constraints are deferred. `digest` is optional in authoring config, but fresh startup records the resolved digest into runtime metadata.

```toml
[[plugins.enabled]]
id = "user:example"
version = "0.1.0"  # optional exact package-version requirement
digest = "sha256:..." # optional pin in authoring, resolved in runtime metadata
config = { level = "concise" }
```

If no digest is pinned in authoring, fresh startup may resolve the newest acceptable discovered package according to explicit policy. Once a Pod is started, the resolved manifest/session metadata should record the exact source-qualified id and digest so restore is stable.

## Permissions and grants

Plugin permission declarations are requests, not grants. Effective grants are the result of Plugin-layer policy combined with existing Yoi authority layers:

- resolved manifest/profile plugin enablement;
- Plugin policy for the source-qualified package id and deterministic digest;
- normal tool permission policy;
- filesystem scope checks;
- web provider enablement and network safety checks;
- secret references and secret-store policy;
- runtime limits for WASM or other execution engines.

The Plugin package permission model must not reuse `pod::feature` HostAuthority or grant concepts. The feature layer is an API/contribution substrate; it is not a security boundary for untrusted plugin packages. Plugin grants need their own explicit policy that can fail closed before a Hook, Tool, WASM host function, provider bridge, or external runtime is exposed.

When a package requests authority outside policy, diagnostics should explain the denied category and package identity without leaking raw secret values, environment contents, full private config, or large plugin-provided text.

## Archive safety and materialization

Archive handling should validate before runtime use:

- Reject absolute paths, `..`, empty segments, Windows drive prefixes, NUL bytes, duplicate normalized paths, and paths that normalize outside the package root.
- Reject symlinks, hardlinks, device files, special files, and entries that are not regular files or directories.
- Enforce bounded extraction: maximum archive size, maximum expanded size, maximum entry count, maximum per-file size, and a compression-ratio limit.
- Validate every manifest-referenced path against the normalized entry set.
- Decode text manifests as UTF-8 and bound diagnostic excerpts.
- Ignore or normalize archive metadata such as mtimes, owners, groups, and executable bits; these should not affect runtime authority.

After validation, compute a deterministic digest over the normalized materialized package, not over incidental ZIP ordering or timestamps. A stable digest input should include the format version, normalized relative path, file length, and file content hash for each regular file in sorted order.

Runtime should materialize packages into a digest-keyed cache, for example:

```text
<cache>/plugins/sha256-<hex>/
  plugin.toml
  module.wasm
  ...
```

Initialization should read from the digest-keyed cache, not directly from the mutable user/workspace store. This makes restore, diagnostics, and lock/pin behavior reproducible.

Optional lock behavior can be added in a later Ticket:

- an authoring-time pin in Profile/manifest configuration;
- a workspace lock file recording source-qualified id, version, source store, digest, and selected package path;
- restore metadata that records the actual digest used by the Pod.

A lock or pin is selection authority, not execution authority. Enablement and grants are still required.

## Diagnostics

Diagnostics should be safe, bounded, and attributable:

- Include source-qualified id when available, source kind, validation phase, and digest when computed.
- Prefer canonical store-relative paths or redacted absolute paths; avoid dumping large path lists.
- Never print raw secret values, provider tokens, environment dumps, or plugin-supplied opaque payloads.
- Treat package metadata and README text as untrusted content when showing it to an LLM or UI.
- Report discovery errors without disabling unrelated valid packages.

## Runtime notes

Declarative hooks are data contributions. Loading a declarative hook still requires explicit package enablement. Hook text should enter the system through the normal Hook/Worker paths, preserving the rule that model-affecting inputs are committed to history before they affect context when applicable.

WASM packages should initialize only from the digest-keyed cache after enablement and grant resolution. The host should use a narrow ABI, bounded memory, fuel/time limits, bounded output, and explicit host functions. A WASM module must not inherit filesystem, network, tool, secret, process, or MCP authority from the package store path.

Tool contributions from plugins should pass through the normal ToolRegistry and permission checks. Plugin-provided schemas can describe arguments, but schema presence is not permission to execute a tool.

## MCP boundary

MCP remains a separate feature-backed integration and is out of the initial Plugin package runtime. A `.yoi-plugin` package must not launch an MCP server or imply MCP enablement.

A future MCP/plugin bridge would need its own Ticket covering external process authority, lifecycle, permission mapping, resource/prompt operations, diagnostics, and trust model. Until then, package metadata may mention compatibility for humans, but runtime packaging should ignore it.

## Follow-up implementation cuts

Good follow-up Tickets are intentionally separable:

1. Manifest/Profile plugin enablement schema and resolved-session metadata, including restore behavior and digest pins.
2. Package discovery for builtin, user, and project stores with source-qualified identity and collision diagnostics.
3. `.yoi-plugin` archive validation, deterministic digest computation, and digest-keyed cache materialization.
4. Plugin-layer permission policy that combines package requests with existing tool/scope/web/secret/runtime allowlists without using `pod::feature` HostAuthority concepts.
5. Declarative hook package loading from enabled, materialized packages.
6. WASM package ABI, initialization limits, host-function grants, and Tool/Hook contribution plumbing.
7. Optional lock-file or pin update workflow for reproducible fresh startup.
8. Future MCP/plugin bridge, only if explicitly approved as a separate design and implementation effort.
