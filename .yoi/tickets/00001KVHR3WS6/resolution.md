## Resolution

`00001KVHR3WS6` を完了しました。

実装内容:
- MCP `tools/list` protocol result/tool types と bounded pagination helper を `crates/mcp` に追加しました。
- MCP stdio discovery feature module を `crates/pod` に追加しました。
- Configured stdio server を initialize し、bounded `tools/list` を呼び、server-provided tool metadata を untrusted data として検証・正規化して ToolRegistry contribution path に登録します。
- Tool names は server namespace を含む stable namespaced name（例: `Mcp_<server>_<tool>`）に正規化されます。
- Invalid schema、duplicate/colliding normalized names は bounded diagnostics で fail-closed になります。Collision 時は該当 normalized identity は model-visible tool になりません。
- Server metadata / annotations / instructions は Yoi instructions, scope, permissions, system/developer instructions を弱める authority として扱いません。
- Registration は existing protocol-provider / ToolRegistry contribution path を通ります。
- This Ticket は `tools/call` execution を実装していません。Registered discovery-only stub は explicit not-implemented error を返し、MCP `tools/call` は送信しません。
- Resources/prompts/list_changed は実装していません。

主な commit:
- `66fa9d55 mcp: register stdio server tools`
- `0080c5b3 mcp: reject colliding tool names`
- `a1f904b8 merge: mcp tool registration`

Review:
- r1 は duplicate/colliding normalized MCP tool names が diagnostic-only で fail-closed でないため `request_changes`。
- Coder が collision handling を修正し、該当 identity が model-visible にならない test を追加。
- r2 は `approve`。

最終 validation:
- `cargo fmt --check`
- `git diff --check HEAD^1..HEAD`
- `cargo test -p mcp list_tools --test stdio_lifecycle`
- `cargo test -p pod feature::mcp --lib`
- `cargo test -p mcp`
- `cargo check -p pod -p mcp`
- `nix build .#yoi --no-link`

Package impact:
- `nix path-info -S .#yoi`: `113089912`

Validation log:
- `/run/user/1000/yoi/yoi-orchestrator/bash-output/bash-SnBew4.log`