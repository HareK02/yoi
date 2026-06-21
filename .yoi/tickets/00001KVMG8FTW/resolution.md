Plugin host API の one-shot outbound request capability を `host_api.https` / `grants.https` から URL permission based `host_api.request` に置き換え、Orchestrator worktree の `orchestration` branch に統合した。

主な成果:
- Active API / docs / WIT naming を `request` に移行。
- Manifest に `host_api.request` と `[[request]]` target declaration を追加。
- Enablement grant を request target grant として扱うよう変更。
- Runtime authorization を manifest-declared request target と enabled request grant の両方が URL/method/scheme/host/port/path coverage で許可する場合のみ network I/O に進む形にした。
- Grant-only / missing-grant / broad / partial-coverage states を static inspection と `yoi plugin show` diagnostics で区別。
- Broad/covering grant と broad manifest + narrower grant の intersection semantics を runtime と static inspection で一致させた。
- Loopback/local/private target は ambient ではなく、URL host declaration + grant に基づく明示 authority として扱う方針を docs に記録。
- Embedded credentials、credential-like headers、WebSocket URLs/upgrades、SSE/event-stream requests を reject/unsupported にした。
- Old `host_api.https` / `grants.https` / `PluginHttps*` / old WIT names は active code/docs/resources から削除。
- Focused manifest / pod / yoi plugin CLI tests を追加・更新。

統合・検証:
- Merge commit: `8a15cca5 merge: plugin request host api`
- Implementation commits: `962b7699`, `0e14e7c1`
- Reviewer final verdict: approve
- Validation passed: `cargo fmt --check`, `git diff --check HEAD^1..HEAD`, `cargo test -p manifest request --quiet`, `cargo test -p pod feature::plugin::tests --lib --quiet`, `cargo test -p yoi plugin_cli::tests --quiet`, `cargo check -p manifest -p pod -p yoi --quiet`, stale active naming grep, `cargo run -p yoi -- ticket doctor`, and `nix build .#yoi --no-link`。

範囲外:
- Regex URL target matching は追加していない。
- WebSocket/SSE/persistent connection support は `host_api.request` に含めていない。WebSocket は別 capability / design Ticket 側で扱う。