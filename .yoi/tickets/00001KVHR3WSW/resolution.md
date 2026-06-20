## Resolution

`00001KVHR3WSW` を完了しました。

実装内容:
- MCP `notifications/tools/list_changed`, `notifications/resources/list_changed`, `notifications/prompts/list_changed` を bounded kind-only state として記録します。
- Notification params は保存・render・behavior に使わず、hidden resource/prompt context injection を防止します。
- Safe-boundary refresh 用の snapshot/clear API を追加しました。
- Startup tool discovery では、registration 前に `tools/list_changed` が観測された場合のみ `tools/list` を最大 1 回 refresh します。
- Refresh 後も変更が続く場合は bounded restart-required diagnostic を出し、active-run model-visible tool schema を post-registration mutation しません。
- MCP tool/resource/prompt operations 中に list_changed が観測された場合、ordinary Tool output に bounded warning を明示的に返します。
- Resource/prompt notifications は content fetch/injection を行わず、explicit list/read/get tools でのみ扱います。
- Sampling / elicitation / remote transport は実装していません。

主な commit:
- `e33dee19 mcp: handle list changed notifications`
- `ae5f3e42 merge: mcp list changed handling`

Review:
- r1 は `approve`。
- Reviewer は current-run schema/history invariants、safe-boundary refresh、restart-required fallback、notification params の非使用、no hidden injection、no sampling/elicitation/remote scope creep を確認しました。

最終 validation:
- `cargo fmt --all --check`
- `git diff --check HEAD^1..HEAD`
- `cargo test -p mcp list_changed -- --nocapture`
- `cargo test -p pod mcp::tests:: -- --nocapture`
- `cargo test -p mcp`
- `cargo check --workspace`
- `nix build .#yoi --no-link`

Package impact:
- `nix path-info -S .#yoi`: `113428296`

Validation log:
- `/run/user/1000/yoi/yoi-orchestrator/bash-output/bash-ddp5Ei.log`