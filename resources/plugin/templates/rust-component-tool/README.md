# Rust Component Tool Template

This is the embedded starter template for a Yoi Component Model Tool Plugin written with the first-party Rust PDK.

## What this template demonstrates

- `wasm-component` runtime targeting `yoi:plugin/tool@1.0.0`.
- Guest-side runtime binding setup through the PDK.
- Typed JSON input parsing through `run_json_tool` via `export_component_tool!`.
- Typed JSON output serialization with `ToolOutput::json`.
- Structured, bounded `ToolError` output for user-visible Tool failures.

The PDK is guest-side only. It does not grant filesystem, network, or environment authority. Host-side Plugin manifests and grants remain the authority boundary for Tool execution and host APIs.

## Checkout/development dependency

Inside the Yoi checkout this template uses a local path dependency and declares an empty `[workspace]` so it can be checked in place without becoming a member of Yoi's root workspace:

```toml
yoi-plugin-pdk = { path = "../../../../crates/plugin-pdk" }
```

If this template is copied to an independent Plugin repository, pin a Yoi source revision with `rev` instead of tracking a branch. Use the repository root `.git` URL, not the browser `/src/branch/...` URL:

```toml
yoi-plugin-pdk = { git = "https://gitea.hareworks.net/Hare/yoi.git", package = "yoi-plugin-pdk", rev = "<pinned-yoi-commit-sha>" }
```

`plugin.component.wasm` in the template is a text placeholder so `yoi plugin check` and `yoi plugin pack` can exercise deterministic local package validation immediately. Replace it with a real built component before enabling or executing the Plugin.

## Next steps

1. Replace package/plugin ids, names, descriptions, and Tool schema.
2. Replace `EchoInput` / `EchoOutput` and `handle_echo` with your Tool logic.
3. Build the Rust component artifact for `wasm32-unknown-unknown`, replacing the placeholder `plugin.component.wasm`.
4. Run `yoi plugin check .` and `yoi plugin pack . --output ./my-plugin.yoi-plugin`.
5. Copy the package to a Plugin store and add explicit enablement with pinned digest/grants after review.
