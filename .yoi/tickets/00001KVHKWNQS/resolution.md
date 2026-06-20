## Resolution

`00001KVHKWNQS` を完了しました。

実装内容:
- `yoi plugin new rust-component-tool <path-or-name>` を追加しました。
- `yoi plugin check <path-or-package> [--json]` を追加しました。
- `yoi plugin pack <path> [--output <file>] [--json]` を追加しました。
- Safe directory/package reading、deterministic digesting、deterministic `.yoi-plugin` writing、symlink/root-escape rejection を含む materialized package helpers を追加しました。
- `check` / `pack` は Plugin code を実行せず、既存 static Plugin inspection を再利用して manifest/runtime/schema/permission/host API declarations を検査します。
- Embedded Rust Component Tool template を `new` で利用し、generated template を check/pack できるよう placeholder `plugin.component.wasm` を追加しました。
- Placeholder artifact は `check` で検出され、generated template / packed archive は `partial` と bounded diagnostic を返します。placeholder が残る間は enablement-ready guidance を出しません。
- `plugin new` は existing destination symlink を拒否し、write-through を防ぎます。
- JSON report shape、human output、CLI help/docs を更新しました。
- Focused tests と CLI smoke coverage を追加しました。

主な commit:
- `945ecdf6 plugin: add authoring cli`
- `699db538 plugin: harden authoring checks`
- `87704ad4 merge: plugin authoring cli`

Review:
- r1 は destination symlink write-through と placeholder artifact の enablement-ready 表示で `request_changes`。
- Coder が symlink refusal、placeholder detection、`partial` status/diagnostics、tests/docs を追加。
- r2 は `approve`。

最終 validation:
- `cargo fmt --check`
- `git diff --check HEAD^1..HEAD`
- `cargo check -p yoi`
- `cargo test -p yoi plugin_cli`
- `cargo test -p yoi-plugin-pdk template`
- `nix build .#yoi --no-link`

補足:
- 初回 `nix build .#yoi --no-link` は `aws-lc-sys` build 中に `No space left on device` で environment failure。
- Orchestrator worktree の Cargo build artifacts を `cargo clean` で削除してから再実行し、Nix build は成功しました。
- `nix path-info -S .#yoi`: `112260512`

Validation log:
- `/run/user/1000/yoi/yoi-orchestrator/bash-output/bash-Q0KE3A.log`