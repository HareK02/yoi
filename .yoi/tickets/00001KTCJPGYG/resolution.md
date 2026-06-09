Ticket role Pod launcher is complete and merged.

Implementation:

- `4bf0e27 feat: add ticket role pod launcher`
- `dd70517 fix: harden ticket role launch execution`
- merge commit: `3d6c1ab merge: add ticket role launcher`

Summary:

- Added `crates/client/src/ticket_role.rs` as a reusable client-level Ticket role launch layer.
- Added launch planning for fixed Ticket roles using `.yoi/ticket.config.toml`:
  - intake
  - orchestrator
  - coder
  - reviewer
  - investigator
- Kept TUI free from `pod` internals; TUI can use `client`.
- Generated first-run input as `Segment::WorkflowInvoke` plus `Segment::Text`.
- Kept Profile responsible for durable system/role behavior.
- Did not add role-level `system_instruction` support.
- Exposed unresolved `launch_prompt` refs in plans/text without treating them as system instructions.
- Added execution API using `spawn_pod`, `PodClient`, and `Method::Run` with acceptance confirmation.
- Top-level execution now rejects `profile = "inherit"` with `UnsupportedInheritProfile` rather than passing invalid `--profile inherit` semantics.
- Run delivery waits for acceptance evidence (`UserMessage`, `InvokeStart UserSend`, or `TurnStart`) and reports error/close/timeout.

Review:

- External sibling review initially requested changes for two blockers:
  1. invalid top-level execution of `inherit` profile;
  2. no first-run acceptance confirmation.
- Both blockers were fixed in `dd70517`.
- Re-review approved with no blockers.

Non-blocker follow-ups:

- Add fake-socket/client execution tests for acceptance/rejection/close/timeout behavior.
- Add aggregate prompt/list caps; current implementation bounds individual fields but not list length globally.
- TUI/CLI integration should surface `UnsupportedInheritProfile` clearly or require concrete role profiles until an inheritance-aware launch path exists.

Post-merge validation passed:

- `cargo test -p client ticket`
- `cargo test -p ticket`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `git diff --check`
- `./tickets.sh doctor`
- `nix build .#yoi --no-link`

This clears the prerequisite for `tui-ticket-role-actions`.
