Implemented explicit Ticket workflow state.

Final frontmatter fields:
- `workflow_state: intake | ready | queued | inprogress | done`
- `attention_required: null | "..."`
- `queued_by: null | "..."`
- `queued_at: null | "..."`

State/default behavior:
- Closed Tickets default/derive to `done` where appropriate.
- Existing non-closed Tickets without explicit workflow state use conservative defaults without treating labels/title/thread heuristics as workflow-state authority.
- Transient activity such as reviewing/reworking/validating is not persisted in frontmatter.

Workflow transition APIs/tools:
- `mark_intake_ready` / `TicketIntakeReady` performs `intake -> ready`, appending typed `intake_summary` and `state_changed` events.
- `queue_ready` remains the dedicated panel Queue path for `ready -> queued`, sets queued metadata, and appends typed `state_changed`.
- `set_workflow_state` / `TicketWorkflowState` is bounded to role-side transitions `queued -> inprogress` and `inprogress -> done`.
- Generic `set_state_field(..., "workflow_state", ...)` is rejected to prevent bypass.
- Backward/skip transitions such as `ready -> inprogress`, `queued -> done`, and `done -> intake` are rejected.

Panel changes:
- Panel rows display explicit workflow state directly.
- Ticket rows are simplified to state + slug/id + title.
- Pod rows are simplified to pod-state + pod-name.
- Row operations move to selected-row actionbar/key hints instead of permanent action/status/phase columns.
- Queue replaces the previous Go/ApproveIntake wording and is only valid for current `workflow_state == ready`.
- Queue notifies Orchestrator when reachable; notification failure does not roll back a successful Ticket transition.
- No-Ticket Pod-centric panel behavior is preserved.

Role prompt/tool behavior:
- Intake/Orchestrator role text now uses `workflow_state` / `Queue` vocabulary.
- Intake is instructed to set `workflow_state = ready` through typed Ticket tools after materializing a Ticket.
- Orchestrator treats `queued` as schedulable and moves to `inprogress` when starting.

Validation after merge:
- `cargo test -p ticket workflow --lib`
- `cargo test -p ticket`
- `cargo test -p tui workspace_panel --lib`
- `cargo test -p tui multi_pod --lib`
- `cargo test -p yoi panel`
- `cargo test -p yoi ticket`
- `cargo test -p pod ticket --lib`
- `cargo test -p client ticket_role`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `git diff --check HEAD~1..HEAD`
- `cargo build -p yoi`
- `target/debug/yoi ticket doctor`
- `nix build .#yoi --no-link --print-out-paths`

External review approved after transition-graph enforcement was added.
