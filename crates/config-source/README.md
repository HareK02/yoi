# config-source

Shares the virtual Workspace configuration source tree, Decodal evaluation, diagnostics, and completion across native/WASM. Inputs are content-addressed snapshots, not a mechanism for reading host configuration directories. Callers own persistence, save-time authorization, and whole-tree CAS.

## Main entry points

- [`VirtualPath`] and [`ConfigTreeSnapshot`] — virtual paths and source-change validation
- [`WorkspaceConfigSchemaBundle`] and [`ToolchainContract`] — evaluation-condition identity
- [`SnapshotEnvironment`] and [`SnapshotImportLoader`] — evaluation and closed import resolution

## Decodal 0.4.1 adoption

The evaluator, language service and formatter are pinned together. The Decodal version participates in both schema-bundle and toolchain fingerprints; updating a dependency is not permission to reinterpret a saved evaluation.

Workspace Server keeps saved 0.4.0 source bytes, fingerprints and projection evidence readable without reevaluating them on load or startup. Ordinary evaluation of that old contract requires an explicit owner-authorized configuration refresh. Use the existing configuration adoption API: read `/api/w/{workspace}/config/adoption`, retain its editing head, catalog digest and source digest, preview an adoption request with `changes: []`, then commit that same request with a stable operation ID. If evaluation fails, repair the source in the normal candidate flow; do not relabel stored fingerprints. The adoption transaction appends the new evaluation/history/receipt and retains old evidence. Exact replay of an old receipt never reactivates its old evaluation. No live database is migrated merely by building this crate.

## Unicode and native/WASM regression

`resources/config-schema/unicode-values.json` is shared by native and generated-WASM tests. It covers the original `retained 雪` failure, accents, emoji, combining characters, escapes, Unicode relative imports, Markdown imports and multibyte diagnostic positions. Regex constraints remain disabled in the current feature selection; that case checks the existing `unsupported_feature` diagnostic, not newly enabled regex evaluation.

To compare actual runtimes (including complete diagnostics and formatter results):

```sh
mkdir -p target
cargo test --locked -p config-source -p config-source-wasm
cargo run --locked -q -p config-source --example unicode_parity > target/unicode-native.json
cargo build --locked -p config-source-wasm --target wasm32-unknown-unknown --release
# Use wasm-bindgen CLI 0.2.117, matching Cargo.lock; wasm std and lld are required.
wasm-bindgen --target web --out-dir web/workspace/src/lib/workspace/config-source/generated target/wasm32-unknown-unknown/release/config_source_wasm.wasm
cd web/workspace
deno test --allow-read --allow-env test/config-source/wasm-parity.test.ts
deno run --allow-read test/config-source/unicode-native-parity.ts ../../target/unicode-native.json
```
