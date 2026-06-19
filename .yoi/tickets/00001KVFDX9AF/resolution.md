Ticket `00001KVFDX9AF` is complete.

Completed implementation:
- Added granted outbound HTTPS host API for WASM Plugin Tools.
- Added typed `host_api.https` grant scope with host, method, optional path prefix, and bounded request/response options.
- Implemented `yoi:https` WASM host import handling.
- Enforced grant/allowlist checks before network access.
- Enforced HTTPS-only behavior and rejected `http://`, embedded credentials, localhost/private/link-local/local targets, IPv4-mapped/compatible IPv6 private/local forms, and unsafe DNS results.
- Bound DNS validation to the actual reqwest connection path by pinning validated public socket addresses with `resolve_to_addrs`.
- Added request/response bounds, timeout, no redirects, `no_proxy()`, response truncation, and secret-like diagnostics redaction.
- Preserved ordinary Tool result/history path and avoided hidden context injection.
- Updated Plugin CLI inspection and manifest/permission model to expose HTTPS host API grant/diagnostic details.

Reviewed / merged:
- Implementation commits:
  - `7377527f` (`plugin: implement https host api`)
  - `85683f17` (`plugin: harden https target validation`)
- First review requested changes for IPv4-mapped IPv6 bypass and DNS validation TOCTOU.
- Re-review approved with no remaining blockers.
- Orchestrator merge commit: `6beb8625` (`merge: plugin https host api`)

Validation in Orchestrator worktree:
- `cargo fmt --check` — passed
- `cargo check -p pod -p manifest -p yoi` — passed
- `cargo test -p pod feature::plugin::tests -- --nocapture` — passed; 39 passed, 0 failed
- `cargo test -p manifest plugin -- --nocapture` — passed; 17 passed, 0 failed
- `cargo test -p yoi plugin_cli -- --nocapture` — passed; 10 passed, 0 failed
- `git diff --check` — passed
- `nix build .#yoi --no-link` — passed

Cleanup:
- Stopped Coder Pod `yoi-coder-00001KVFDX9AF`.
- Stopped Reviewer Pod `yoi-reviewer-00001KVFDX9AF-r2`.
- Removed child worktree `/home/hare/Projects/yoi/.worktree/00001KVFDX9AF-plugin-https-host-api`.
- Deleted merged branch `impl/00001KVFDX9AF-plugin-https-host-api`.

Root/original workspace was not read/written/merged/validated for this Ticket, per Panel Queue instruction. The completed work is integrated on the Orchestrator branch.