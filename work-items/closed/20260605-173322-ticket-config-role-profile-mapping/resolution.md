Ticket config role profile mapping is complete and merged.

Implementation:

- `767870a ticket: add workspace ticket config`
- `8fab67b ticket: reject nix profile selectors`
- merge commit: `9910df4 merge: add ticket config roles`

Summary:

- Added `.yoi/ticket.config.toml` support through `crates/ticket/src/config.rs`.
- Added fixed Ticket roles only:
  - `intake`
  - `orchestrator`
  - `coder`
  - `reviewer`
  - `investigator`
- Added role config fields:
  - `profile`
  - optional `launch_prompt`
  - optional `workflow`
- Did not add role-level `system_instruction`; durable role/system behavior remains owned by the selected Profile.
- Added backend config for local Ticket storage:
  - `kind = "local"`
  - `root = "work-items"`
- Missing config defaults to local `<workspace>/work-items`, all role profiles `inherit`, no launch prompts, and documented workflow defaults.
- Unknown roles/fields, unsupported backend kinds, malformed refs, path-like profile refs, `.lua`, and `.nix` profile refs are rejected.
- Wired the configured backend root into `crates/pod/src/feature/builtin/ticket.rs`.
- Preserved fail-closed/no-register behavior for malformed config or unusable backend roots.
- Kept dependency direction clean: `pod -> ticket`; `ticket` does not depend on `pod` or `manifest`.

Review:

- External sibling review initially requested one blocker fix: reject `*.nix` profile selectors to align with `SpawnPod.profile` validation.
- Coder fixed it in `8fab67b` and added a focused `legacy.nix` rejection test.
- Re-review approved with no blockers.

Non-blocker follow-ups:

- Normalize `HostAuthority::TicketBackend { root }` and canonical backend root consistently before relying on root strings for stricter security/audit comparison.
- Add an execution-level Pod adapter test that runs a Ticket tool against the configured backend root.
- Later role launcher should resolve Profile selectors/prompt refs/workflow refs at runtime and commit launch prompts as user/task messages.

Post-merge validation passed:

- `cargo test -p ticket`
- `cargo test -p pod ticket --lib`
- `cargo test -p pod feature --lib`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `git diff --check`
- `./tickets.sh doctor`
- `nix build .#yoi --no-link`
