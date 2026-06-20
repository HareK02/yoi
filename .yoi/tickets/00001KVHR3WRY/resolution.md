## Resolution

`00001KVHR3WRY` を完了しました。

実装内容:
- New internal crate `mcp` を追加しました。
- Explicit MCP stdio server config から resolved stdio server spec を作成する bridge を追加しました。
- Tokio child process による local stdio MCP server lifecycle foundation を実装しました。
- stdin/stdout/stderr handling、newline-delimited JSON-RPC request/response handling、initialize/capability negotiation、`notifications/initialized` を実装しました。
- stdout/stderr/protocol payloads は bounded に扱います。
- stderr は bounded diagnostics/logging として扱い、protocol failure とは別扱いです。
- server name / phase-aware errors を追加しました。
- shutdown は stdin close / wait / terminate / kill fallback で deterministic に行います。
- Server-to-client requests は fail-closed し、sampling/elicitation は advertise せず、unknown request は JSON-RPC error で返します。
- `McpStdioServerSpec` の `Debug` は custom redacted 実装にし、resolved env/secret-derived values を出さない regression test を追加しました。
- ToolRegistry / tools/resources/prompts registration、remote MCP / Streamable HTTP / OAuth は実装していません。

主な commit:
- `a114fa9d mcp: implement stdio lifecycle client`
- `f396e1a2 mcp: redact stdio server spec debug`
- `9cf5344f merge: mcp stdio lifecycle client`

Review:
- r1 は resolved spec `Debug` による env/secret leak で `request_changes`。
- Coder が custom redacted `Debug` と regression test を追加。
- r2 は `approve`。

最終 validation:
- `cargo fmt --check`
- `git diff --check HEAD^1..HEAD`
- `cargo test -p mcp`
- `cargo check`
- `cargo tree -p mcp --depth 1`
- `nix build .#yoi --no-link`

Package impact:
- `nix path-info -S .#yoi`: `112615056`