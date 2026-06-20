# Plugin development

This guide is for building a Yoi Plugin outside the Yoi runtime codebase. It describes the current Plugin package shape, how to author a Tool Plugin, how to enable it in a workspace, and how to inspect/debug it.

Yoi Plugins are intentionally explicit:

- putting a package in `.yoi/plugins` only makes it discoverable;
- a Profile/config entry must explicitly enable it;
- Plugin grants must allow its surfaces and host APIs;
- Plugin code runs only through the configured sandbox runtime;
- Tool calls and Tool results use the ordinary Yoi Tool/Worker history path.

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
- read-only `yoi plugin list/show` inspection.

Still intentionally separate/future work:

- `yoi plugin new/check/pack` authoring commands;
- multi-language SDK/PDK crates;
- Service / Ingress surfaces;
- WebSocket or inbound HTTP for bidirectional bridges;
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

## Component Model + Rust PDK authoring

Component Model authoring with `yoi-plugin-pdk` is the preferred path for new Tool Plugins. The raw core-Wasm ABI remains available only as compatibility/transitional runtime support.

Yoi's Component Model Tool world is stored in `resources/plugin/wit/`. The embedded Rust starter template is available as data in the Yoi source tree at:

```text
resources/plugin/templates/rust-component-tool/
```

It contains:

- `Cargo.toml` with a checkout-local `yoi-plugin-pdk` path dependency;
- `src/lib.rs` with WIT binding generation and typed JSON Tool handling;
- `plugin.toml` targeting `kind = "wasm-component"` and `world = "yoi:plugin/tool@1.0.0"`;
- README next steps and the future out-of-tree pinned git `rev` dependency pattern.

A minimal PDK-backed Rust sketch is also available at:

```text
docs/examples/plugin-component-tool/lib.rs
```

The important authoring shape is:

```rust
use serde::{Deserialize, Serialize};
use yoi_plugin_pdk::{ToolContext, ToolError, ToolOutput};

yoi_plugin_pdk::wit_bindgen::generate!({
    world: "tool",
    path: "../../../resources/plugin/wit",
});

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

`run_json_tool` parses the WIT `input-json` string into a typed input, passes a `ToolContext` containing the selected Tool name, and serializes `ToolOutput` JSON accepted by the current component runtime. `ToolError` values are structured and bounded, then rendered through the ordinary Tool result path; the component cannot inject hidden context.

The PDK is guest-side only. It does not depend on Yoi host/runtime crates and does not grant filesystem, network, or environment authority. Host-side Plugin manifests and explicit enablement grants remain the authority boundary for Tool execution and for WIT host APIs such as `yoi:host/https` and `yoi:host/fs`.

The exact component build pipeline depends on the authoring toolchain (`wit-bindgen`, component adapter tooling, etc.). Crates.io publication, remote template fetching, and `yoi plugin new/check/pack` are intentionally deferred. Until they exist, Plugin authors should treat the template/example as the ABI contract sketch and use `yoi plugin list/show` plus focused runtime tests to verify packages.

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

## `https` host API

The `https` host API is outbound-only and grant-gated. It is meant for Tool calls such as webhook posting or REST requests. It is not a WebSocket/Gateway or inbound HTTP bridge.

Manifest permissions should request `host_api.https` in addition to the Tool permissions. Enablement grants must then allow the API and constrain hosts/methods.

Example grant shape:

```toml
[plugins.enabled.grants]
permissions = [
  { kind = "surface", surface = "tool" },
  { kind = "tool", name = "discord_post" },
  { kind = "host_api", api = "https" },
]

[[plugins.enabled.grants.https]]
host = "discord.com"
methods = ["POST"]
path_prefixes = ["/api/webhooks/"]
```

Yoi rejects `http://`, localhost/private/link-local targets, disallowed hosts/methods, oversize requests/responses, and missing grants. Credentials must come from explicit config/secret references, not ambient environment variables.

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

## Outbound vs bridge integrations

After `https`, an outbound Discord webhook Tool is feasible:

```text
Yoi Tool call -> Plugin Tool -> yoi:host/https -> Discord REST/webhook
```

A bidirectional Discord bridge is different. It needs a Service surface plus Ingress and either WebSocket/Gateway support or inbound HTTP interactions:

```text
Discord Gateway/Webhook -> Plugin Service/Ingress -> host routing policy -> notify/run/drop/diagnostic
```

Do not model bidirectional bridge work as an `https` Tool alone.

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
