# Delegation intent: Ticket role Pod launcher

## Intent

Implement a reusable Ticket role Pod launcher so TUI and later CLI/orchestrator surfaces can launch fixed Ticket-role Pods without duplicating profile/config/workflow/prompt construction logic.

The launcher should use `.yoi/ticket.config.toml` fixed role configuration, generate first-run task content, and keep dynamic Ticket/action context in `Method::Run` input rather than hidden context injection.

## Worktree / branch

- worktree: `/home/hare/Projects/yoi/.worktree/ticket-role-pod-launcher`
- branch: `work/ticket-role-pod-launcher`

## Requirements

- Support fixed Ticket roles only:
  - `intake`
  - `orchestrator`
  - `coder`
  - `reviewer`
  - `investigator`
- Load `.yoi/ticket.config.toml` through `ticket::config::TicketConfig`.
- Use role `profile` selector as the child Pod profile selector.
- Use role `workflow` ref as model-visible workflow input in the first run.
- Generate first committed user/task message content from a typed launch context.
- Keep selected Profile responsible for durable system/role behavior; do not add `system_instruction` support.
- Do not inject dynamic instructions into context outside history; first-run prompt/task content must go through `Method::Run`.
- Prefer a client-level API so TUI can use it without depending on `pod` crate internals.
- Avoid duplicating current runtime spawn internals if existing `client::spawn_pod`, `PodClient`, and `protocol::Method::Run` can be used cleanly.
- Expose a launch planning API even if full execution is constrained, so TUI work has a stable boundary.

## Suggested module placement

Preferred:

- `crates/client/src/ticket_role.rs`
- exports from `crates/client/src/lib.rs`

Rationale:

- `tui` already depends on `client`.
- `client` can depend on `ticket` without introducing `tui -> pod`.
- `client` owns host-side spawn/socket mechanics.

If current crate boundaries make full execution awkward, implement the pure planning API in `client` first and clearly report the execution gap.

## Suggested API shape

Exact names can change, but keep the surface typed:

```rust
pub enum TicketRoleLaunchKind {
    Intake,
    Orchestrator,
    Coder,
    Reviewer,
    Investigator,
}

pub struct TicketRoleLaunchContext {
    pub workspace_root: PathBuf,
    pub role: TicketRole,
    pub pod_name: Option<String>,
    pub ticket: Option<TicketRefLike>,
    pub user_instruction: Option<String>,
    pub intent_packet: Option<String>,
    pub worktree_path: Option<PathBuf>,
    pub branch: Option<String>,
    pub validation: Vec<String>,
    pub report_expectations: Vec<String>,
}

pub struct TicketRoleLaunchPlan {
    pub role: TicketRole,
    pub pod_name: String,
    pub profile: String,
    pub workflow: String,
    pub launch_prompt_ref: Option<String>,
    pub run_segments: Vec<protocol::Segment>,
}
```

Use existing `ticket::config::TicketRole` if practical rather than duplicating role enum. Avoid exposing pod internals.

## Prompt generation expectations

Generated first-run text should include:

- role name;
- Ticket id/slug if present;
- user/action instruction;
- workflow slug;
- launch_prompt ref if configured but unresolved;
- intent packet if provided;
- worktree path / branch if provided;
- validation/report expectations if provided;
- reminder that Profile supplies system/role behavior and the Workflow supplies process.

Prefer typed `Segment::WorkflowInvoke` plus text if current protocol/client path supports it. If not, include workflow slug in text and document the limitation.

## Non-goals

- TUI command/action UI.
- Stateful workflow engine.
- Phase-specific prompts/tool gating.
- Role-level `system_instruction` support.
- Prompt resource resolution if it requires moving prompt loader APIs across crates.
- Changing Profile resolution semantics.
- Changing `SpawnPod` tool semantics in the `pod` crate.
- Scheduler/lease/queue automation.
- Worktree creation automation.

## Validation

Run at least:

- `cargo test -p client ticket` or focused client tests;
- `cargo test -p ticket` if touched;
- `cargo check --workspace --all-targets`;
- `cargo fmt --check`;
- `git diff --check`;
- `./tickets.sh doctor`.

Run `nix build .#yoi --no-link` if feasible.

## Completion report

Report:

- worktree path / branch;
- commit hash;
- final module/API layout;
- whether launch execution is implemented or only planning;
- generated prompt / workflow segment behavior;
- how role profile config is used;
- validation results;
- unresolved risks/follow-ups;
- whether `tui-ticket-role-actions` can proceed.
