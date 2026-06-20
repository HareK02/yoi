Plugin Service/Ingress component lifecycle surface を実装し、Orchestrator worktree の `orchestration` branch に統合した。

主な成果:
- Pod plugin feature に host-managed `PluginInstanceRegistry` / instance handle 境界を追加し、Tool dispatch を instance 経由に変更。
- New instance-capable component world `yoi:plugin/instance@1.0.0` と WIT resource を追加。
- `yoi-plugin-pdk` と Rust component template に instance-oriented authoring support を追加。
- Existing component Tool world / raw wasm Tool runtime を instance registry compatibility path に維持。
- Manifest/static validation に Service / Ingress declarations と per-surface grant validation を追加。
- Service lifecycle/status/diagnostics と bounded in-process ingress dispatch path を実装。
- Tool / Service / Ingress enabled-surface filtering を runtime install, dispatch guard, and resolved static inspection / `yoi plugin list/show` に適用。
- `plugin check` は package declaration inspection、resolved `plugin list/show` は selected/enabled surfaces に基づく reporting に分離。
- Focused tests added for manifest validation, legacy Tool compatibility, instance state persistence, ingress dispatch, Service/Ingress grant denial, failure diagnostics, and partial enabled-surface static reporting。

統合・検証:
- Merge commit: `43c9216e merge: plugin instance lifecycle surface`
- Implementation commits: `147a6005`, `870bcc76`, `79ca0f7f`, `627c8f36`
- Reviewer final verdict: approve
- Validation passed: `cargo fmt --check`, `git diff --check HEAD^1..HEAD`, `cargo test -p manifest plugin -- --nocapture`, `cargo test -p pod plugin -- --nocapture`, `cargo test -p yoi plugin -- --nocapture`, `cargo check -p yoi`, `cargo check -p yoi-plugin-pdk`, template cargo-check, `yoi ticket doctor`, and `nix build .#yoi --no-link`。

範囲外:
- Discord Bridge 本体、public registry/install/update/signature tooling、arbitrary Plugin UI channel、hidden context injection、Service/Ingress による model-visible Tool schema mutation は実装していない。