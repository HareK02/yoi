Ticket `00001KVFDX9AY` is complete.

Completed implementation:
- Added granted scoped filesystem `fs` host API for WASM Plugin Tools.
- Added `PluginFsGrant { root, operations }` and `PluginFsOperation::{read,list,write}` to the Plugin grant model.
- Implemented `yoi:fs` WASM host imports for read/list/write with response buffer handling.
- Enforced Plugin-specific grants before filesystem access; Plugin Tools do not inherit Pod/workspace filesystem authority.
- Added path safety for relative-only requests, traversal rejection, symlink/root escape rejection, canonical root checks, and Unix `O_NOFOLLOW` for writes.
- Added read/list/write bounds, explicit truncation/fail-closed behavior, write serialization by normalized/canonical target, and bounded/redacted diagnostics.
- Updated Plugin CLI inspection to show configured fs grants.

Reviewed / merged:
- Implementation commit: `717c0999` (`plugin: implement fs host api`)
- Reviewer result: approve, no blockers.
- Reviewer noted a non-blocking future hardening context: the current implementation is path/canonicalization based and does not fully eliminate adversarial external TOCTOU races where another local process mutates granted-root directories between checks and open/read/list/write. This is accepted within the feasible scope of the initial subset.
- Orchestrator merge commit: `c94e157b` (`merge: plugin fs host api`)

Validation in Orchestrator worktree:
- `cargo fmt --check` — passed
- `cargo check -p pod -p manifest -p yoi` — passed
- `cargo test -p pod feature::plugin -- --nocapture` — passed; 48 passed, 0 failed
- `cargo test -p manifest plugin -- --nocapture` — passed; 17 passed, 0 failed
- `cargo test -p yoi plugin_cli -- --nocapture` — passed; 10 passed, 0 failed
- `git diff --check` — passed
- `nix build .#yoi --no-link` — passed

Cleanup:
- Stopped Coder Pod `yoi-coder-00001KVFDX9AY`.
- Stopped Reviewer Pod `yoi-reviewer-00001KVFDX9AY`.
- Removed child worktree `/home/hare/Projects/yoi/.worktree/00001KVFDX9AY-plugin-fs-host-api`.
- Deleted merged branch `impl/00001KVFDX9AY-plugin-fs-host-api`.

Root/original workspace was not read/written/merged/validated for this Ticket, per Panel Queue instruction. The completed work is integrated on the Orchestrator branch.