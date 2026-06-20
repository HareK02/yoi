## Resolution

`00001KVHKWNQA` を完了しました。

実装内容:
- Guest-side Rust PDK crate `yoi-plugin-pdk` を追加しました。
- PDK は typed JSON input/output helper、bounded `ToolError`、`ToolContext`、`run_json_tool` 系 helper、`wit_bindgen` re-export、`export_component_tool!` macro を提供します。
- PDK は host/runtime Yoi crates に依存せず、authority を付与しません。Host-side Plugin manifest grants が Tool execution / host API use の authority boundary のままです。
- Embedded Rust Component Tool template を `resources/plugin/templates/rust-component-tool/` に追加しました。
- Template は local checkout/dev path dependency を使い、future out-of-tree git `rev` pattern を docs に記録しています。
- `resources/plugin/wit` を `wit-bindgen` が parse できる package layout に修正し、host WIT dependency を `resources/plugin/wit/deps/yoi-host/yoi-host-v1.wit` に移動しました。
- WIT keyword `list` は `%list` escape にし、import name semantics を保持しました。
- Embedded template は empty `[workspace]` により in-tree standalone package として check できます。
- `wit_bindgen::generate!` を実際に `resources/plugin/wit` に対して実行する probe と、embedded template の `wasm32-unknown-unknown` cargo-check probe を追加しました。
- Plugin development docs / design docs / package docs / example source を更新しました。
- `yoi plugin new/check/pack`、remote template fetch、crates.io publication、full packaged component execution はこの Ticket の non-goals / follow-up として残しました。

主な commit:
- `06287aca plugin: add rust pdk template`
- `0a9e585c plugin: fix rust pdk wit template probes`
- `edc53a6b merge: plugin rust pdk templates`

Review:
- r1 は WIT parse failure と embedded template Cargo workspace issue で `request_changes`。
- Coder が WIT layout / `%list` / template `[workspace]` / actual probes を追加。
- r2 は `approve`。

最終 validation:
- `cargo fmt --check`
- `git diff --check HEAD^1..HEAD`
- `cargo test -p yoi-plugin-pdk`
- `cargo test -p manifest embedded_rust_component_tool_template_is_valid_package_shape`
- `cargo test -p pod pdk_tool_output_shape_is_accepted_by_wasm_decoder`
- `cargo check`
- `cargo tree -p yoi-plugin-pdk --edges normal`
- `cargo check --manifest-path resources/plugin/templates/rust-component-tool/Cargo.toml --target wasm32-unknown-unknown`
- `nix build .#yoi --no-link`

Package impact:
- `nix path-info -S .#yoi`: `112156384`

Validation log:
- `/run/user/1000/yoi/yoi-orchestrator/bash-output/bash-o9gvGb.log`