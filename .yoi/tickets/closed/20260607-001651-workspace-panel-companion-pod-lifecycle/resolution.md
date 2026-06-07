Implemented, reviewed, merged, and validated.

Summary:
- Added a real workspace Companion Pod lifecycle for `yoi panel`.
- Companion Pod id/name is resolved from the workspace directory basename; for this repository it is exactly `yoi`.
- No `-companion` suffix or separate companion-specific id was introduced for the normal workspace Companion.
- The panel reuses live Companion Pods and restores restorable Companion Pods before spawning a missing Companion.
- The workspace Orchestrator remains a separate identity such as `yoi-orchestrator`.
- Panel close does not stop the Companion.
- The `Companion` composer target now sends user text to the workspace Companion Pod through `Method::Run`, not to arbitrary selected Pods.
- Empty composer behavior still preserves selected-Pod open/attach behavior.
- Busy/unavailable Companion states preserve the draft and surface bounded diagnostics.
- Ticket Intake remains separate and does not append Intake text to Companion history.
- Companion is not modeled as a Ticket role and does not use `.yoi/ticket.config.toml` role slots.
- No selected-Pod direct send, `--multi`, or `:ticket` surface was reintroduced.

Implementation:
- Child commit: `ba0c9d5 feat: wire panel companion lifecycle`
- Merge commit: `merge: companion pod lifecycle`

Review:
- External reviewer `companion-lifecycle-reviewer-20260607` approved with no blockers.

Validation after merge:
- `cargo test -p tui companion --lib`
- `cargo test -p tui multi_pod --lib`
- `cargo test -p tui workspace_panel --lib`
- `cargo test -p tui --lib`
- `cargo fmt --check`
- `git diff --check`
- `target/debug/yoi ticket doctor`
