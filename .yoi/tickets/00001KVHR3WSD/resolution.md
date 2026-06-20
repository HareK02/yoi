## Resolution

`00001KVHR3WSD` を完了しました。

実装内容:
- MCP `tools/call` typed request/result/content types を追加しました。
- `McpStdioClient::call_tool(...)` を追加しました。
- MCP discovered tool の discovery-only stub を executable `McpStdioTool` に置き換えました。
- Execution は configured stdio MCP server を spawn/initialize し、`tools/call` を送信して shutdown します。
- Permission denial は ordinary Worker `PreToolCall` path により Tool execution 前に適用されるため、denied call は MCP server に送信されません。
- Results は ordinary Tool result/history path を通ります。Hidden context injection はありません。
- Normal MCP result、MCP `isError: true`、JSON-RPC protocol error を区別しました。
- MCP content / structuredContent / `_meta` / rich output は untrusted data として bounded に serialization されます。
- Image/audio data は raw payload を落とし、size metadata のみ残します。
- Resources/read、prompts/get、list_changed、sampling、elicitation は実装していません。

主な commit:
- `9a245403 mcp: execute stdio tool calls`
- `399a9d43 merge: mcp tools call`

Review:
- r1 は `approve`。
- Reviewer は permission-before-call、ordinary Tool result/history path、`isError` と protocol error の区別、bounded/untrusted result handling、out-of-scope surface が無いことを確認しました。

最終 validation:
- `cargo fmt --check`
- `git diff --check HEAD^1..HEAD`
- `cargo test -p mcp --test stdio_lifecycle`
- `cargo test -p pod feature::mcp`
- `cargo check -p mcp -p pod`
- `nix build .#yoi --no-link`

Package impact:
- `nix path-info -S .#yoi`: `113196368`

Validation log:
- `/run/user/1000/yoi/yoi-orchestrator/bash-output/bash-lkjYsX.log`