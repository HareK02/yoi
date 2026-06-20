# Rust Component Tool Template

This is the embedded starter template for a Yoi Component Model Tool Plugin written with the first-party Rust PDK.

## What this template demonstrates

- `wasm-component` runtime targeting `yoi:plugin/tool@1.0.0`.
- Guest-side WIT binding generation through the PDK's `wit_bindgen` re-export.
- Typed JSON input parsing through `run_json_tool` via `export_component_tool!`.
- Typed JSON output serialization with `ToolOutput::json`.
- Structured, bounded `ToolError` output for user-visible Tool failures.

The PDK is guest-side only. It does not grant filesystem, network, or environment authority. Host-side Plugin manifests and grants remain the authority boundary for Tool execution and host APIs.

## Checkout/development dependency

Inside the Yoi checkout this template uses a local path dependency:

```toml
yoi-plugin-pdk = { path = "../../../../crates/plugin-pdk" }
```

If this template is copied elsewhere before crates.io publication exists, pin a Yoi source revision instead of fetching an unpinned remote template:

```toml
yoi-plugin-pdk = { git = "https://github.com/example/yoi.git", package = "yoi-plugin-pdk", rev = "<pinned-yoi-revision>" }
```

Crates.io publication, remote template fetching, and `yoi plugin new/check/pack` are intentionally deferred to later authoring-tooling work.

## Next steps

1. Replace package/plugin ids, names, descriptions, and Tool schema.
2. Replace `EchoInput` / `EchoOutput` and `handle_echo` with your Tool logic.
3. Build a component for `wasm32-unknown-unknown` with the Component Model tooling used by your environment.
4. Package `plugin.toml` and `plugin.component.wasm` into a `.yoi-plugin` archive.
5. Use `yoi plugin list` / `yoi plugin show` plus focused runtime tests to inspect and validate the package.

The exact component build/pack command is not part of this template yet because deterministic `yoi plugin new/check/pack` authoring commands are a separate planned Ticket.
