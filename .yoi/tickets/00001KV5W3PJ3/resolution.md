Ticket `00001KV5W3PJ3` is complete.

Completed implementation:
- Added typed Plugin permission declarations/grants for tool surfaces, tool names/namespaces, `external_write`, and future `host_api.https` / `host_api.fs` boundaries.
- Bound grants to source-qualified package identity, deterministic digest, and exact package version.
- Added fail-closed registration gating in `PluginToolFeature::install`.
- Added independent runtime execution gating in `run_plugin_wasm_tool` before WASM load/execute.
- Added future host API permission boundary checks without implementing actual `https` / `fs` host APIs.
- Added bounded/sanitized denial diagnostics.
- Preserved the existing PreToolCall / Tool permission path; plugin grants are an additional fail-closed gate, not an ambient authority grant.

Reviewed / merged:
- Implementation commit: `b1ba1599` (`plugin: enforce permission grants`)
- Reviewer result: approve, no blockers.
- Orchestrator merge commit: `94aa3c1d` (`merge: plugin permission grants`)

Validation in Orchestrator worktree:
- `cargo fmt --check` — passed
- `cargo check -p manifest -p pod` — passed
- `cargo test -p pod plugin -- --nocapture` — passed; 27 passed, 0 failed
- `cargo test -p manifest plugin -- --nocapture` — passed; 17 passed, 0 failed
- `git diff --check` — passed

Cleanup:
- Stopped Coder Pod `yoi-coder-00001KV5W3PJ3`.
- Stopped Reviewer Pod `yoi-reviewer-00001KV5W3PJ3`.
- Removed child worktree `/home/hare/Projects/yoi/.worktree/00001KV5W3PJ3-plugin-permission-grants`.
- Deleted merged branch `impl/00001KV5W3PJ3-plugin-permission-grants`.

Root/original workspace promotion was not performed in this step; the completed work is integrated on the Orchestrator branch.