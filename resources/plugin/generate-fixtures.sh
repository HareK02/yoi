#!/usr/bin/env bash
# Regenerate authoring copies and real, never-instantiated Components from Rust.
# Prerequisites: Cargo/Rust with wasm32-unknown-unknown std and lld on PATH.
set -euo pipefail
root="$(cd -- "$(dirname -- "$0")/../.." && pwd)"
cd "$root"
for template in rust-component-tool rust-component-https; do
  cp resources/plugin/wit/yoi-plugin-v2.wit "resources/plugin/templates/$template/wit/yoi-plugin-v2.wit"
  cp resources/plugin/wit/deps/yoi-host/yoi-host-v2.wit "resources/plugin/templates/$template/wit/deps/yoi-host/yoi-host-v2.wit"
done
cargo test --locked -p yoi-plugin-pdk --target-dir target/sdk/test --test template
cp target/sdk/fixtures/rust-component-tool/plugin.component.wasm crates/plugin-pdk/tests/fixtures/tool-v2.component.wasm
cp target/sdk/fixtures/rust-component-https/plugin.component.wasm crates/plugin-pdk/tests/fixtures/https-tool-v2.component.wasm
