# Plugin development

This guide is for building a Yoi Plugin outside the Yoi runtime codebase. It describes the current Plugin package shape, how to author a Tool Plugin, how to enable it in a workspace, and how to inspect/debug it.

Yoi Plugins are intentionally explicit. The Plugin system is designed around the following host-side principles:

- package discovery is inventory only; putting a package in `.yoi/plugins` does not enable, register, or execute it;
- a Profile/config entry must explicitly enable each Plugin package by source-qualified id, version, and digest;
- Plugin grants must allow each surface and host API before registration or execution can use it;
- Plugin code runs only through the configured sandbox runtime;
- Plugin packages do not inherit Pod workspace filesystem, network, environment, or Ticket authority;
- Tool calls and Tool results use the ordinary Yoi Tool/Worker history path;
- Plugin metadata, output, and diagnostics are untrusted unless Yoi host policy says otherwise.

## Design intent

Yoi's Plugin platform is meant to make extension behavior reviewable before it becomes model-visible. A Plugin package should answer four separate questions:

1. **What is this package?** `plugin.toml` declares identity, version, runtime, surfaces, requested permissions, and Tool schemas.
2. **Is it enabled here?** Workspace/Profile config chooses exact package refs and pinned digests.
3. **What may it do?** Plugin grants authorize Tool surfaces and host APIs such as `https` and `fs`.
4. **How does it interact with the model?** Tool schemas/results enter through ordinary ToolRegistry and Tool history paths.

Keep these layers separate when designing a Plugin. Do not make package discovery imply enablement. Do not make SDK/PDK convenience imply authority. Do not treat Rust helper APIs or host API wrappers as permission grants. The host always re-checks authority at registration/execution/API-call boundaries.

Yoi's preferred Plugin shape is **Tool first**. A good Tool Plugin has a narrow schema, deterministic input/output behavior, explicit side-effect metadata, and a minimal grant set. Long-running services, inbound events, and autonomous routing are future Service/Ingress work; they should not be hidden inside a Tool package.

Component Model authoring is the preferred path for new Plugins. The raw core-Wasm ABI exists for compatibility and tests, but authors should use the Rust PDK/template unless they are deliberately testing the low-level runtime.

## Current status

Implemented foundation:

- package discovery from project/user Plugin stores;
- explicit enablement resolution;
- Tool surface registration;
- Plugin permission grants;
- raw core-Wasm Tool runtime;
- Component Model Tool runtime;
- first-party Rust PDK helpers for Component Model Tool guests;
- embedded Rust Component Tool starter template;
- `https` and `fs` host APIs for Tool runtime;
- read-only `yoi plugin list/show` inspection;
- local first-party authoring commands: `yoi plugin new`, `yoi plugin check`, and `yoi plugin pack`.

Still intentionally separate/future work:

- multi-language SDK/PDK crates;
- Service / Ingress surfaces;
- WebSocket or inbound HTTP for bidirectional external event integrations;
- public registry/install/update/signature tooling.

## Package locations

Yoi discovers `.yoi-plugin` packages from:

```text
<workspace>/.yoi/plugins/*.yoi-plugin
${XDG_DATA_HOME:-~/.local/share}/yoi/plugins/*.yoi-plugin
```

Use project packages for workspace-specific Plugins and user packages for personal reusable Plugins. Project packages should normally be committed only when the package content is safe and intended to be part of the project.

## Package archive format

A `.yoi-plugin` package is currently a bounded ZIP archive. For now, create it with stored entries, not compressed entries:

```bash
(cd my-plugin && zip -0 -r ../example.echo.yoi-plugin plugin.toml plugin.component.wasm)
```

The archive root must contain `plugin.toml`. Runtime files referenced by the manifest must also be inside the archive. Yoi rejects path traversal, root escapes, malformed manifests, unsupported API/runtime versions, and other unsafe archive shapes.

## Authoring CLI

Use the local authoring commands for first-party deterministic authoring. These commands never fetch remote templates, never run Plugin code, never mutate enablement configuration, and never generate or embed secrets.

Create a Rust Component Tool starter from embedded resources:

```bash
yoi plugin new rust-component-tool ./my-plugin
```

`new` writes only inside the requested destination and refuses an existing non-empty destination or destination symlink. The generated template includes `plugin.toml`, Rust source, Cargo metadata, README next steps, and a placeholder `plugin.component.wasm` artifact so local `check`/`pack` validation can run immediately. Replace the placeholder with a real built component before enabling or executing the Plugin.

Validate a source directory or an existing `.yoi-plugin` archive:

```bash
yoi plugin check ./my-plugin
yoi plugin check ./my-plugin --json
yoi plugin check ./my-plugin.yoi-plugin --json
```

`check` performs bounded static validation of the directory/archive shape, manifest, runtime declaration, referenced artifact presence, Tool schemas, permission declarations, host API declarations, archive safety, and deterministic digest when a package can be materialized. Component-world validation is metadata-only: it verifies the declared world string and runtime manifest shape, but it does not instantiate or execute the component. A generated placeholder component produces `status = "partial"` plus a diagnostic and is not enablement-ready until replaced. Invalid checks print the same structured report and exit non-zero.

Pack a source directory into a deterministic stored `.yoi-plugin` archive:

```bash
yoi plugin pack ./my-plugin
yoi plugin pack ./my-plugin --output ./my-plugin.yoi-plugin --json
```

`pack` rejects malformed manifests, missing runtime artifacts, symlinks/root escapes, and unsupported package shapes. The JSON output contains the stable package reference, output path, digest, entries, and safety flags. After review, copy the package to `.yoi/plugins/` (or the user Plugin store) and add explicit Profile/config enablement with pinned digest and grants; packing and checking do not do this for you.

## Designing a Plugin

Design a Plugin around the smallest reviewable contract that is useful to the model.

For Tool Plugins:

- expose one clear operation per Tool name;
- keep the input schema narrow and explicit;
- make side effects visible in the Tool name, description, and `external_write` / permission metadata;
- request only the host APIs needed for that Tool;
- prefer deterministic, structured output over conversational prose;
- return bounded summaries and content that are useful as Tool results;
- avoid hiding long workflows, background daemons, or inbound event handling inside a Tool call.

A Tool should be a capability the model may choose to call, not a second agent runtime. If the desired behavior needs a long-lived connection, incoming events, or autonomous routing, treat that as future Service/Ingress design rather than stretching the Tool surface.

Design package permissions as a review surface. A reviewer should be able to read `plugin.toml` plus the enablement grants and understand:

- what Tools become model-visible;
- what external side effects are possible;
- what hosts or paths can be touched;
- what data can flow back into ordinary Tool results.

## Manifest: `plugin.toml`

A minimal Component Model Tool Plugin manifest looks like this:

```toml
schema_version = 1
id = "example.echo"
name = "Example Echo"
version = "0.1.0"
surfaces = ["tool"]
permissions = [
  { kind = "surface", surface = "tool" },
  { kind = "tool", name = "example_echo" },
]

[runtime]
kind = "wasm-component"
component = "plugin.component.wasm"
world = "yoi:plugin/tool@1.0.0"

[[tools]]
name = "example_echo"
description = "Echo input text."
input_schema = { type = "object", properties = { text = { type = "string" } }, required = ["text"], additionalProperties = false }
external_write = false
```

The preferred new runtime is `wasm-component`. The older raw core-Wasm runtime remains explicit for compatibility:

```toml
[runtime]
kind = "wasm"
entry = "plugin.wasm"
abi = "yoi-plugin-wasm-1"
```

Do not rely on package presence to activate anything. Discovery only records inventory.

## Rust PDK authoring

Rust authoring with `yoi-plugin-pdk` is the preferred path for new Tool Plugins. The raw core-Wasm ABI remains available only as compatibility/transitional runtime support.

Create a starter with:

```bash
yoi plugin new rust-component-tool ./my-plugin
```

The generated package contains:

- `Cargo.toml` with a checkout-local `yoi-plugin-pdk` path dependency;
- `src/lib.rs` with the runtime binding setup and typed JSON Tool handling;
- `plugin.toml` targeting `kind = "wasm-component"`;
- README next steps and the out-of-tree pinned git `rev` dependency pattern.

For an independent Plugin repository, replace the checkout-local path dependency with a pinned Yoi source revision. Use the repository root `.git` URL, not the browser `/src/branch/...` URL, and pin `rev` instead of tracking a moving branch:

```toml
[dependencies]
serde = { version = "1.0", features = ["derive"] }
yoi-plugin-pdk = { git = "https://gitea.hareworks.net/Hare/yoi.git", package = "yoi-plugin-pdk", rev = "<pinned-yoi-commit-sha>" }
```

As a Plugin author, treat the generated binding setup as template code. Edit the typed input/output structs and handler function rather than hand-writing runtime ABI glue.

The important authoring shape is:

```rust
use serde::{Deserialize, Serialize};
use yoi_plugin_pdk::{ToolContext, ToolError, ToolOutput};

#[derive(Deserialize)]
struct EchoInput {
    text: String,
}

#[derive(Serialize)]
struct EchoOutput<'a> {
    tool: &'a str,
    text: String,
}

fn handle_echo(ctx: ToolContext, input: EchoInput) -> Result<ToolOutput, ToolError> {
    ToolOutput::json(
        format!("{} ok", ctx.tool_name()),
        EchoOutput {
            tool: ctx.tool_name(),
            text: input.text,
        },
    )
}

yoi_plugin_pdk::export_component_tool!(Plugin, handle_echo);
```

The PDK parses the runtime input string into a typed Rust value, passes a `ToolContext` containing the selected Tool name, and serializes `ToolOutput` JSON accepted by the current component runtime. `ToolError` values are structured and bounded, then rendered through the ordinary Tool result path; the component cannot inject hidden context.

The PDK is guest-side only. It does not depend on Yoi host/runtime crates and does not grant filesystem, network, or environment authority. Host-side Plugin manifests and explicit enablement grants remain the authority boundary for Tool execution and for host APIs such as `https` and `fs`.

The expected authoring flow is Rust-first: generate the starter, edit `src/lib.rs`, replace the local path dependency with a pinned `git` + `rev` dependency when the Plugin lives outside the Yoi checkout, build the Rust component artifact for `plugin.component.wasm`, run `yoi plugin check`, then `yoi plugin pack`. Crates.io publication and remote template fetching are intentionally deferred. Use `yoi plugin list/show` to inspect the packaged/enabled state before trying to execute the Tool.

## Enabling a Plugin in a workspace

Enablement belongs in the resolved Profile/config path for the workspace. For local dogfooding or private experiments, use the ignored local overlay rather than committing secrets or local paths:

```toml
# .yoi/override.local.toml

[features]
plugins = true

[[plugins.enabled]]
id = "project:example.echo"
version = "0.1.0"
digest = "sha256:<digest from yoi plugin show/list>"
surfaces = ["tool"]

[plugins.enabled.grants]
id = "project:example.echo"
version = "0.1.0"
digest = "sha256:<same digest>"
permissions = [
  { kind = "surface", surface = "tool" },
  { kind = "tool", name = "example_echo" },
]
```

A source-qualified id is preferred:

```text
project:example.echo
user:example.echo
builtin:example.echo
```

Unqualified ids can be ambiguous and should fail closed when more than one source matches.

## Inspecting Plugins

Use the read-only CLI inspection commands first:

```bash
yoi plugin list
yoi plugin list --json
yoi plugin show project:example.echo
yoi plugin show project:example.echo --json
```

`list/show` must not execute Plugin code. They are intended to explain static state:

- discovered packages;
- enabled vs disabled packages;
- missing packages referenced by enablement;
- invalid manifests;
- digest/version/source mismatches;
- granted/denied permissions;
- Tool registration eligibility;
- runtime metadata.

Typical statuses:

```text
active    enabled and statically valid for at least one surface/tool
disabled  discovered but not explicitly enabled
missing   enablement references a package that is not discovered
rejected  invalid manifest, incompatible API, digest mismatch, grant denial, etc.
partial   usable package with some rejected surfaces/tools
```

## `request` host API

The `request` host API is a one-shot outbound HTTP request API. It is meant for bounded Tool calls such as JSON POSTs or REST requests. It is not a WebSocket, SSE/event-stream, gateway, daemon, or inbound HTTP surface; persistent transports require a separate Plugin capability.

Manifest permissions should request `host_api.request` in addition to the Tool permissions, and the package manifest must statically declare the URL targets it may call. Enablement grants must then allow the API and grant matching request targets. A grant without a matching manifest target is unsafe/unused and is shown as ineligible rather than expanding authority.

Example manifest shape:

```toml
permissions = [
  { kind = "surface", surface = "tool" },
  { kind = "tool", name = "http_post_json" },
  { kind = "host_api", api = "request" },
]

[[request]]
scheme = "https"
host = "api.example.com"
methods = ["POST"]
path_prefixes = ["/v1/"]
```

Example enablement grant shape:

```toml
[plugins.enabled.grants]
permissions = [
  { kind = "surface", surface = "tool" },
  { kind = "tool", name = "http_post_json" },
  { kind = "host_api", api = "request" },
]

[[plugins.enabled.grants.request]]
scheme = "https"
host = "api.example.com"
methods = ["POST"]
path_prefixes = ["/v1/"]
```

Yoi checks method, scheme, host, optional port, and path prefix against both the manifest declaration and enablement grant before any network I/O. `http://localhost`, loopback, private, and other local targets are never ambient; they require an explicit manifest request target and an explicit matching grant. The explicit request target is the declared URL authority; a granted DNS hostname may resolve to a loopback/private address without requiring a separate literal-IP grant, so reviewers should grant hostnames only when that resolution behavior is intended. Broad targets such as `host = "*"` are supported only as visibly broad request permissions in inspection/diagnostics. Embedded credentials, credential-like headers, oversize requests/responses, WebSocket URLs/upgrades, and SSE/event-stream requests are rejected.

## `websocket` host API

The `websocket` host API is a separate grant-gated capability named `host_api.websocket`, not an extension of `host_api.request`. It opens host-owned WebSocket connections only when both the package manifest and enablement config declare matching targets. Plugin code drives the lifecycle explicitly through `open`, `send-text`, `recv`, and `close`; incoming messages are returned only from bounded `recv` calls and are not injected into model context, history, Dashboard state, or Ticket state.

Example manifest shape:

```toml
permissions = [
  { kind = "surface", surface = "tool" },
  { kind = "tool", name = "gateway_step" },
  { kind = "host_api", api = "websocket" },
]

[[websocket]]
scheme = "wss"
host = "gateway.example.com"
path_prefixes = ["/gateway"]
```

Example enablement grant shape:

```toml
[plugins.enabled.grants]
permissions = [
  { kind = "surface", surface = "tool" },
  { kind = "tool", name = "gateway_step" },
  { kind = "host_api", api = "websocket" },
]

[[plugins.enabled.grants.websocket]]
scheme = "wss"
host = "gateway.example.com"
path_prefixes = ["/gateway"]
```

Yoi checks scheme (`ws`/`wss`), host, optional port, and path prefix against both declarations before opening the connection. Loopback/private/local targets are not ambient; they require explicit matching manifest and grant entries. Broad WebSocket targets such as `host = "*"` are reported as broad WebSocket diagnostics. v1 is text-only: `send-text` requires UTF-8, binary receive fails closed, guest-supplied handshake headers and embedded URL credentials are rejected, and SecretRef-based credential/header injection is future work. The host bounds open descriptors, text/message size, receive timeout, connection count, handle lifetime, and cleanup on close/instance stop/drop.

## `fs` host API

The `fs` host API is Plugin-scoped and grant-gated. Plugins do not inherit the Pod/workspace filesystem authority automatically.

Example grant shape:

```toml
[plugins.enabled.grants]
permissions = [
  { kind = "surface", surface = "tool" },
  { kind = "tool", name = "read_notes" },
  { kind = "host_api", api = "fs" },
]

[[plugins.enabled.grants.fs]]
root = "/absolute/path/to/plugin-data"
operations = ["read", "list"]
```

Yoi normalizes paths, rejects `..` traversal, rejects symlink/root escapes, and applies read/write/list bounds. Diagnostics must not include file contents.

## Development checklist

1. Create a package directory with `plugin.toml` and the runtime artifact.
2. Build the Wasm/component artifact.
3. Package with stored ZIP entries as `.yoi-plugin`.
4. Put it under `.yoi/plugins/` or the user Plugin store.
5. Run `yoi plugin list` and `yoi plugin show <ref>`.
6. Add explicit enablement and grants.
7. Re-run `yoi plugin show <ref>` until status/diagnostics are correct.
8. Start Yoi with `features.plugins = true` in the resolved config/Profile.
9. Call the Tool and verify ordinary Tool result/history behavior.

## Safety rules for Plugin authors

- Do not assume ambient filesystem, network, or environment access.
- Do not put secrets in `plugin.toml` or package files.
- Request only the minimal host APIs and grants needed.
- Keep Tool output bounded and structured.
- Prefer Component Model authoring for new Plugins.
- Treat raw core-Wasm ABI support as transitional compatibility.
