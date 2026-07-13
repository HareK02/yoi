Runtime observation snapshots now prefer the live execution backend snapshot. The real Worker backend returns a WorkerHandle-built Snapshot containing session-log entries, greeting, status, and in-flight state; the Runtime falls back to the old stub only when no live backend handle exists.

Validation:
- cargo fmt --check
- cargo check -q
- cargo test -q -p worker-runtime --features fs-store,ws-server
- cargo test -q -p yoi-workspace-server
- cd web/workspace && deno task check && deno task test
- git diff --check
- nix build .#yoi --no-link
