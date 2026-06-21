URL permission based Plugin WebSocket host API を実装し、Orchestrator worktree の `orchestration` branch に統合した。

主な成果:
- `host_api.websocket` を `host_api.request` とは別 capability として追加。
- Manifest `[[websocket]]` target declaration と enablement `grants.websocket` を追加し、request targets/grants とは独立させた。
- Static inspection / `yoi plugin show` が WebSocket requested/granted/missing/grant-only/broad diagnostics を request diagnostics とは別に表示するようにした。
- Runtime connect は manifest target と enablement grant の両方が URL を許可する場合のみ network I/O に進む。
- URL checks cover scheme (`ws`/`wss`), host, port, and path prefix。
- Local/private/loopback WebSocket targets は ambient ではなく、明示 declaration + grant が必要。
- Host-owned WebSocket handle API を追加: open, send_text / send-text, recv, close。
- Text-only / explicit bounded receive とし、binary receive は fail closed / unsupported。
- Guest arbitrary handshake headers / embedded credentials を reject。
- Request API は WebSocket/SSE/persistent attempts を引き続き reject。
- Open path は pre-dial capacity reservation と bounded async `tokio-tungstenite` open under `tokio::time::timeout` により max-open / timeout semantics を network I/O 前から enforce。
- Reservation cleanup on open failure / failed commit を追加。
- WIT resource `yoi:host/websocket@1.0.0` と docs を更新。
- `tungstenite`, `tokio-tungstenite`, `futures-util` dependencies と `Cargo.lock` / `package.nix` cargo hash を更新。

統合・検証:
- Merge commit: `354f1e10 merge: plugin websocket host api`
- Implementation commits: `4c1b8c3d`, `ce62d235`, `a766048f`
- Reviewer final verdict: approve
- Validation passed: `cargo fmt --check`, `git diff --check HEAD^1..HEAD`, `cargo test -p pod websocket`, `cargo test -p manifest websocket`, `cargo test -p yoi render_show_distinguishes_request_grant_statuses_and_broad_targets`, `cargo test -p manifest request_host_api_manifest_and_grant_parse_with_request_names`, `cargo check -p manifest -p pod -p yoi`, `cargo run -p yoi -- ticket doctor`, and `nix build .#yoi --no-link`。

範囲外:
- Discord bridge 本体は実装していない。
- Reconnect/backoff/heartbeat scheduler、hidden context/history injection、Dashboard channel、Ticket mutation、direct model Tool invocation は追加していない。
- SecretRef-based credential injection は future follow-up。