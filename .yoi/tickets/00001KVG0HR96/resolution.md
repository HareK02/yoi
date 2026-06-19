## Resolution

`00001KVG0HR96` を完了しました。

実装内容:
- Plugin manifest/runtime metadata に明示的な Component Model runtime (`kind = "wasm-component"`) を追加しました。
- 既存 raw core-Wasm runtime (`kind = "wasm"`, `abi = "yoi-plugin-wasm-1"`) は明示的に維持しました。
- `wasmtime::component` による Component Tool execution path を追加しました。
- Component Tool は既存 ToolRegistry / Worker Tool path を通って実行され、hidden context injection はありません。
- WIT host imports は権限そのものではなく、Plugin grants が Tool execution / host API use の authority boundary のままです。
- Component runtime に raw runtime 相当の Wasmtime resource limits を追加し、memory/table/instance/output bound の negative tests を追加しました。
- WASI fs/network/env は expose していません。
- `yoi plugin list/show` static inspection は Component runtime metadata を報告し、component artifact を実行しません。
- WIT files、Component sample authoring sketch、docs/design updates、package/Nix updates を追加しました。
- JSON-string WIT v1 request/response shape は migration bridge として docs に記録し、structured records は follow-up に deferred としました。

主な commit:
- `57bbf14e plugin: implement component model runtime`
- `a705bb3b plugin: bound component model runtime resources`
- `63d7ad78 merge: plugin component model runtime`

Review:
- r1 は Component runtime resource limit 不足で `request_changes`。
- Coder が resource limiter / negative tests / docs note を追加。
- r2 は `approve`。

最終 validation:
- `cargo fmt --check`
- `git diff --check HEAD^1..HEAD`
- `cargo check`
- `cargo test -p pod feature::plugin::tests -- --nocapture`
- `cargo test -p manifest plugin -- --nocapture`
- `cargo test -p yoi plugin -- --nocapture`
- `nix build .#yoi --no-link`

Package impact:
- `nix path-info -S .#yoi`: `112156120`
- `bin/yoi`: `54605944`
- output dir: `53M`

Validation log:
- `/run/user/1000/yoi/yoi-orchestrator/bash-output/bash-rZDseu.log`