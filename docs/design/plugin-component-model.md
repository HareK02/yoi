# Plugin Component Model migration

Yoi's current Plugin Tool runtime uses a narrow core-WebAssembly ABI. That was the right MVP shape because it made sandboxing, bounded input/output, and fail-closed host imports explicit. It should not become the long-term authoring interface.

The preferred direction is to adopt the WebAssembly Component Model for Plugin Tool authoring and host APIs. Component Model adoption means Plugin interfaces are described as typed WIT worlds and lowered through the canonical ABI, instead of every Plugin author or SDK wrapper hand-writing pointer/length memory plumbing.

## What Component Model changes

A core Wasm module exposes low-level functions and memory. Yoi's current Plugin Tool ABI is shaped like this:

```text
export memory
export yoi_tool_call() -> i32
import yoi:tool/tool_name_len() -> i32
import yoi:tool/tool_name_read(ptr, len) -> i32
import yoi:tool/input_len() -> i32
import yoi:tool/input_read(ptr, len) -> i32
import yoi:tool/output_write(ptr, len) -> i32
```

This is small and auditable, but it makes raw ABI details part of the authoring model. A Component Model world can instead describe a typed contract:

```wit
package yoi:plugin;

interface tool {
  record request {
    tool-name: string,
    input-json: string,
  }

  record response {
    output-json: string,
  }

  variant tool-error {
    invalid-input(string),
    denied(string),
    failed(string),
  }

  run: func(req: request) -> result<response, tool-error>;
}

world tool-plugin {
  export tool;
}
```

The exact WIT is still design work, but the important boundary is fixed: the Plugin author sees typed values and generated bindings; the host sees typed imports/exports; Yoi still enforces package enablement and Plugin grants outside the component.

## External patterns considered

Common Wasm extension systems normally ship more than a runtime:

- Extism-style systems provide host runtimes plus language PDKs. Plugin authors write normal typed functions while the PDK hides the raw ABI and host functions remain explicit.
- Spin-style systems combine a manifest, language SDK/templates, default-deny outbound/file capabilities, and Wasm components.
- wasmCloud-style systems separate components from capability providers and connect them through typed interfaces.
- The Component Model standardizes the interface layer with WIT and canonical ABI so host APIs can be versioned and bindings generated across languages.

The shared lesson is that a usable Wasm Plugin system needs a manifest, explicit capabilities, generated or hand-written SDK bindings, examples/templates, inspection tooling, and a versioned ABI. Yoi already has the manifest/discovery/enablement/grant/runtime foundation; the missing long-term piece is the typed component authoring interface.

## Yoi policy

Adopting the Component Model must not change Yoi's authority model:

- Package discovery is inventory only and does not register or execute a Plugin.
- Explicit enablement is required before any Tool surface is registered.
- Plugin grants are required before runtime execution and before `https` / `fs` / future host API calls.
- Component imports are not authority by themselves; host-side grant checks remain authoritative.
- Tool calls and Tool results continue through the ordinary ToolRegistry and Worker history path.
- No hidden context injection is introduced by component imports, resources, prompts, or SDK helpers.
- Plugin SDKs and templates are authoring aids, not trust boundaries.

## Migration shape

Yoi should support Component Model as an explicit runtime kind rather than silently changing existing raw-ABI packages.

Possible manifest direction:

```toml
[runtime]
kind = "wasm-component"
component = "plugin.component.wasm"
world = "yoi:plugin/tool@1.0.0"
```

The current raw core-Wasm runtime can remain explicit during migration:

```toml
[runtime]
kind = "wasm"
entry = "plugin.wasm"
abi = "yoi-plugin-wasm-1"
```

The migration should be phased:

1. Define WIT packages/worlds for Tool Plugin and initial host APIs.
2. Add manifest/schema support for `runtime.kind = "wasm-component"` without executing it during discovery.
3. Add a component runtime backend and typed host import/export binding.
4. Port `https` and `fs` host API designs to WIT-compatible interfaces.
5. Add a Rust PDK/template around the component world.
6. Decide whether the raw ABI remains supported, becomes legacy-only, or is deprecated after examples and tests move.

## Runtime/backend caution

The current implementation uses `wasmi` for core Wasm. Component Model support will likely require a different backend or a significantly richer component adapter path, such as `wasmtime::component` plus generated bindings. That has consequences for binary size, Nix packaging, build time, runtime limits, and sandbox policy. The migration Ticket must measure and validate those effects explicitly.

If a component backend is added, keep it selected by package runtime metadata and Profile/feature policy. Do not make all Plugin packages depend on component execution during discovery or inspection.

## Relationship to pending host APIs

`https` and `fs` host API Tickets should avoid baking in raw pointer/length interfaces as the long-term authoring contract. If they land before the component runtime, implement them in a way that can be represented as WIT records/results later, and document raw ABI wrappers as transitional.

For example, `https` should be modeled as typed request/response data with explicit grant checks for host/method/path/body bounds. `fs` should be modeled as scoped read/list/write operations with path normalization and root-escape rejection. Those concepts translate well to WIT.

## Non-goals

- Component Model adoption does not imply WASI filesystem/network access.
- It does not replace Plugin grants with WIT imports.
- It does not introduce Service, Ingress, WebSocket, or inbound HTTP by itself.
- It does not merge Plugin and MCP. MCP remains a separate untrusted tool/resource/prompt bridge with its own policy.

## Implemented runtime boundary

Plugin Tool packages now select the runtime explicitly in `plugin.toml`:

```toml
[runtime]
kind = "wasm-component"
component = "plugin.component.wasm"
world = "yoi:plugin/tool@1.0.0"
```

The legacy core-Wasm ABI remains explicit and is not reinterpreted as a
component:

```toml
[runtime]
kind = "wasm"
entry = "plugin.wasm"
abi = "yoi-plugin-wasm-1"
```

The component runtime uses `wasmtime::component` and expects the exported world
`yoi:plugin/tool@1.0.0` with a `call(tool-name: string, input-json: string) ->
string` export. The returned string is the same ToolOutput JSON used by the raw
runtime, so registration and execution still flow through the existing
ToolRegistry and Worker Tool-result history path.

Host imports are stable names under `yoi:host/*@1.0.0`; the repository WIT files
live in `resources/plugin/wit/`. Importing `yoi:host/https@1.0.0` or
`yoi:host/fs@1.0.0` is not authority. The runtime checks package grants before
component instantiation and checks again on every host call. No WASI filesystem,
network, environment, or other ambient imports are linked.

Static discovery and `yoi plugin list/show` only parse package manifests and
reported runtime metadata. They do not instantiate or execute the component.
Wrong `world`, missing artifact metadata, missing `call` export, unsupported
imports, or core-Wasm bytes in a component package all fail closed with bounded
Plugin diagnostics or ordinary Tool errors.

See `docs/examples/plugin-component-tool/lib.rs` and the embedded
`resources/plugin/templates/rust-component-tool/` starter for the preferred
Rust PDK authoring path. `yoi-plugin-pdk` is guest-side only: it re-exports
`wit-bindgen`, provides typed JSON input/output helpers, renders bounded
`ToolError` values as ordinary ToolOutput JSON, and does not depend on host
runtime crates or grant authority. Package authors should generate bindings from
`resources/plugin/wit`, build a component artifact, and set the component
runtime metadata above.

### v1 request/response shape

The v1 component world intentionally keeps Tool input, Tool output, and host API
payloads as JSON strings. This is a migration bridge that preserves the existing
ToolOutput schema, Tool history behavior, grant checks, and raw-Wasm host API
semantics while moving package authors onto WIT/canonical ABI bindings.
Structured WIT records for Tool requests/responses/errors and host HTTPS/FS
payloads are deferred to a follow-up API-design step rather than accidentally
omitted.

## Instance lifecycle surface

The first instance-capable world is `yoi:plugin/instance@1.0.0`. It moves
runtime ownership from per-Tool artifact execution to a host-managed
`PluginInstance`. The same instance handles Tool, Service, and Ingress surfaces,
so Plugin state/config/diagnostics can be shared without bypassing Yoi's normal
authority model.

Important boundaries:

- Tool calls still enter through `ToolRegistry` and return ordinary `ToolOutput`
  that is visible in the Worker history path.
- Service and Ingress grants are separate from Tool grants. Sharing an instance
  does not authorize a surface that lacks its own `surface.*` and per-surface
  permission/grant.
- Ingress delivery accepts bounded typed untrusted events and returns explicit
  JSON to the host. It does not call model Tools or mutate LLM context/history.
- Legacy raw-wasm and `yoi:plugin/tool@1.0.0` component packages are adapted
  behind `PluginInstanceRegistry` for compatibility rather than executed through
  a separate authority path.
- Host APIs such as `https` and `fs` remain independently grant-gated and still
  reject ambient filesystem/network authority.
