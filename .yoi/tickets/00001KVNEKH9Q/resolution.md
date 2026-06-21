Workspace backend に local Host / Worker read API を追加し、Web UI に Host / Worker list を表示する変更を統合した。

主な成果:
- Workspace backend に `/api/hosts`, `/api/workers`, `/api/hosts/{host_id}/workers` を追加。
- `/api/runners` placeholder を削除 / Host-Worker naming に移行。
- Local Pod metadata を read-only bridge として Worker domain object に投影。
- Pod は primary domain ではなく `implementation: { kind: "local_pod", pod_name: ... }` として表現。
- Worker response includes bounded `worker_id`, `host_id`, label/pod name, role/profile when known, `workspace_root`, state/status, implementation detail, and diagnostics。
- Missing/unreadable Pod metadata root は process/server failure ではなく empty workers + bounded diagnostics / unavailable capability に degrade。
- Session transcript / tool result / prompt contents / raw session JSONL は読まない。
- Web UI static SPA に Host / Worker list を追加。
- Existing Ticket / Objective canonical workflows remain unchanged。

統合・検証:
- Merge commit: `c884d517 merge: workspace host workers`
- Implementation commit: `58143ead feat: expose workspace hosts and workers`
- Reviewer final verdict: approve
- Validation passed: `cargo fmt --check`, `git diff --check HEAD^1..HEAD`, `cargo test -p yoi-workspace-server`, `cargo check -p yoi-workspace-server`, `deno task check`, `deno task build`, `cargo run -p yoi -- ticket doctor`, and `nix build .#yoi --no-link`。

範囲外:
- Remote/cloud runner registration, scheduling, start/stop/attach/notify operations, and full run-worker correlation were not implemented。
- Frontend remains static SPA and does not own backend business/lifecycle authority。