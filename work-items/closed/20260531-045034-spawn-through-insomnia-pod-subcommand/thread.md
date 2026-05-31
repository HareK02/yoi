<!-- event: create author: tickets.sh at: 2026-05-31T04:50:34Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-05-31T04:51:14Z -->

## Plan

Implementation plan:

1. Find all internal `insomnia-pod` executable resolution paths (`client::spawn_pod`, `SpawnPod`, `RestorePod`/discovery, tests/dev helpers).
2. Introduce a typed runtime command structure with `program` and `prefix_args`, avoiding shell-string parsing.
3. Make the default runtime command for callers running from `insomnia` use `current_exe()` + `pod` prefix args.
4. Preserve explicit override behavior for development/debugging; if the existing env var cannot safely support prefix args, keep it as executable-only and document that it bypasses the unified default.
5. Update tests to assert prefix args are included and existing spawn/restore behavior is unchanged.
6. Leave Nix/package removal of `insomnia-pod` to a follow-up unless this change proves complete and low-risk.


---

<!-- event: implementation_report author: hare at: 2026-05-31T05:25:12Z -->

## Implementation report

Implementation report from coder Pod `spawn-through-insomnia-pod-coder-20260531`:

- Branch: `spawn-through-insomnia-pod-subcommand`
- Commit: `4f622b8` (`cli: spawn pods through insomnia pod`)
- Added new `pod-command` crate with typed `PodRuntimeCommand { program, prefix_args }`.
- Default runtime command now resolves to `current_exe()` + `pod` prefix args, except when running through the temporary legacy `insomnia-pod` wrapper.
- `INSOMNIA_POD_COMMAND` remains executable-only, is not shell-parsed, and bypasses prefix args for development/debug override behavior.
- Migrated internal Pod runtime launches to the typed command:
  - `client::spawn_pod`
  - `SpawnPod` child process creation
  - `PodDiscovery::restore` / `RestorePod`
  - TUI spawn/restore callers through `client::spawn_pod`
- Preserved detached process behavior, process groups, socket prediction/probing, and `INSOMNIA-READY` stderr handshake.
- Left `insomnia-pod` package/output removal for the next phase.

Validation reported by coder:

- `cargo fmt --check` passed
- `cargo test -p pod-command` passed
- `cargo test -p client -p pod-command` passed
- `cargo test -p pod --lib discovery::tests` passed
- `cargo test -p pod --test spawn_pod_test` passed
- `cargo test -p tui parse_pod_subcommand_uses_runtime_mode` passed
- `cargo check -p client -p pod -p tui` passed with existing dead-code warnings
- `./tickets.sh doctor` passed
- `git diff --check` passed

Unresolved follow-ups:

- Remove/demote `insomnia-pod` package/output once packaging/devshell/docs are updated.
- Optional stronger future test: capture actual argv of a mock spawned executable in a default-command call site.


---

<!-- event: review author: hare at: 2026-05-31T05:25:12Z status: approve -->

## Review: approve

External review by reviewer Pod `spawn-through-insomnia-pod-reviewer-20260531`: approve.

Reviewer summary:

- The implementation adds typed `PodRuntimeCommand { program, prefix_args }` command resolution.
- Default runtime command is now current executable plus `pod` prefix args, except for the temporary legacy `insomnia-pod` wrapper path.
- `INSOMNIA_POD_COMMAND` remains executable-only and is not shell-parsed.
- `client::spawn_pod`, `SpawnPod`, and `RestorePod`/discovery restore paths now use the typed command.
- Detached process behavior and `INSOMNIA-READY` handshake are preserved.
- Nix/package removal and crate rename are not mixed into this ticket.

Blockers: none.

Non-blocking follow-up:

- Future test could capture actual argv of a mock spawned executable in a default-command call site. Current tests are adequate for this ticket because the typed command composition and existing spawn behavior are covered.


---

<!-- event: implementation_report author: hare at: 2026-05-31T05:27:03Z -->

## Implementation report

Main workspace validation after merge:

- `cargo fmt --check` passed
- `cargo test -p pod-command` passed
- `cargo test -p client -p pod-command` passed
- `cargo test -p pod --lib discovery::tests` passed
- `cargo test -p pod --test spawn_pod_test` passed
- `cargo test -p tui parse_pod_subcommand_uses_runtime_mode` passed
- `cargo check -p client -p pod -p tui` passed with pre-existing dead-code warnings
- `./tickets.sh doctor` passed
- `git diff --check` passed


---

<!-- event: close author: hare at: 2026-05-31T05:27:04Z status: closed -->

## Closed

Switched internal Pod runtime spawning/restoration to typed `program + prefix_args` command resolution. Default runtime command now uses the current executable plus `pod`, while `INSOMNIA_POD_COMMAND` remains executable-only and not shell-parsed for development/debug overrides. `client::spawn_pod`, `SpawnPod`, and `RestorePod`/discovery restore paths use the typed command; detached process behavior and `INSOMNIA-READY` handshake are preserved. External review approved and validation passed. `insomnia-pod` packaging removal remains the next phase.


---
