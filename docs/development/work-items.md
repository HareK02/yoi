# Tickets and work outcomes

A Ticket is a durable record of natural-language user intent and a concrete work outcome, not a mandatory code/review/merge pipeline. Research, analysis, planning, documentation, operations, and code can all be Ticket work. Requirements, acceptance criteria, and binding decisions govern what must be done; the requested return path governs whether to continue, complete, close, or return results without concluding the Ticket.

Workspace Server control-plane records, append-only Ticket events, and any linked Merge Request evidence are authoritative. Ticket and Objective records live in the Workspace Server database. Repository-local `.yoi/tickets`, `.yoi/objectives`, and config files are ignored legacy input, not a debugging or compatibility backend. Notifications, chat summaries, and Memory are hints to inspect current authority, not proof of completion.

## Concepts

- `Ticket`: durable work record containing intent, requirements, decisions, useful results/evidence, and immutable closure history. Its case state is only `open` or `closed`; open is not readiness, and closed is not necessarily fulfillment.
- `Classification tag`: Workspace-owned stable identity with a mutable display name. Multiple tags classify/filter Tickets; they never substitute for closure reasons, delegation or start authority.
- `Orchestrator handling`: self-reported `waiting` / `inprogress` / `done` on the current assignment row, with waiting reason, progress, next action and result. These are not case states, Worker states, MR judgments or delivery status.
- `Objective`: a first-class medium-term goal with background, strategy, success criteria, and canonical Ticket context links. It neither schedules nor authorizes Ticket work.
- `Task`: session-local progress tracking, not the project record.
- `Assignment`: an authorized delegation of Ticket work to a Worker, independent from its Profile or Flow.
- `IntentPacket`: a bounded natural-language handoff of intent, requirements, binding decisions, latitude, escalation conditions, and validation.
- `Ticket relation`: non-hierarchical project metadata (`depends_on`, `blocks`, `related`, `supersedes`, `duplicate_of`). Inverse views are derived. Relations do not grant resources or start Workers.

One Ticket should describe an independently useful outcome that can be judged on its own terms. Do not convert temporary execution steps or a broad effort's progress container into mandatory product deliverables.

## Entry points and authority

Use the highest-level interface matching the request: Workspace Dashboard/Ticket UI, authenticated Workspace APIs, product `yoi ticket ...` or `yoi objective ...` commands, and typed Worker tools. Inside Workers, use the supplied typed Ticket tools; do not substitute a CLI or direct storage access for missing tools. Maintainers inspect authority through the Workspace API, Server administration surfaces, and database diagnostics, not repository-local Ticket files.

The Server resolves Workspace access, active configuration, and immutable Profile launch material; Runtime does not infer identity or authority from a checkout's `.yoi` files, cwd, or launch prose. The first committed user request supplies bounded action context, not actor identity or new grants. Profile/Flow selection is configuration, not review, merge, or resource authority. Availability of a tool is not authorization to use it for unrelated work.

## Starting and returning Ticket work

`SpawnTicketWorker` starts and atomically assigns a generic Worker with an `initial_request`. Its registered `profile` is optional (omitted/null selects `builtin:ticket-worker`); `flow` and alias-keyed `workdir_attachments` are optional. No repository or Flow is required for non-code work. Choose an explicit Profile when its tools and instructions fit the task; assignment does not depend on choosing the Coder role. Backend validates claims, derives repository attachment access from declared Ticket targets, and preserves explicit ExternalGrant ceilings. Callers cannot request capabilities through attachment selection.

Orchestrator Assign delegates handling and persists initial waiting with the assignment/audit/receipt; no separate ready/Queue operation is required. The stable `workspace_agent:workspace-orchestrator` principal is distinct from the concrete registered Worker. Answers during waiting/ongoing work continue the same assignment; status is explicitly self-reported, never automatically set on delivery or acceptance. An explicit reissue after done creates a new assignment row even for the same principal, preserving prior result/release/replacement history. Reopen and Worker exchange alone create neither a fresh delegation nor execution rights. See [assignment tracking](ticket-assignment-tracking.md) and [delivery/start fences](ticket-assignment-delivery.md).

The guarded start records acceptance only after spawn, initial input, assignment, and attachment finalization. Current Orchestrator starts (including generic Worker spawn and existing-Worker assignment) must carry the observed `orchestrator_assignment_id`. Do not pre-write handling `inprogress` or mistake a case/status update for a launch. On a failed or unknown outcome, reread durable operation evidence before retrying the same operation; never silently spawn a replacement. After acceptance, reread current Worker responsibility and authoritative unfinished work, not merely `current_worker`. Unfinished dependencies are diagnostics for partial work/waiting, not a blanket start refusal, and are not implicitly started; actual authentication, case closure, resource/claim and recovery guards still apply.

Results can be returned in conversation, optionally recorded in a `TicketComment`, optionally published as a Drive document/artifact when that surface is available, or written/published to an authorized repository. Neither comments nor Drive artifacts are universally required. Preserve concise evidence useful to the user without fabricating work, tests, or conclusions. `implementation_report` is optional historical/audit context, not integration readiness or completion authority.

The return choice is independent of result format. Examples:

- Research returned to the human: record handling done with a bounded result; leave the case open when no closure was requested. No MR or universal approval is required.
- Explicit user confirmation before continuing: record waiting with the specific reason and next action. An answer continues the same assignment; receipt alone is not an inprogress self-report.
- Whole request fulfilled and closure requested: separately record handling and a reasoned case close with `completed`. Other closure reasons (not planned, duplicate, unknown, or custom) do not claim fulfillment.
- Code requiring independent review before merge: use the trusted review path and guarded MR integration. A Profile, Flow transition, tool invocation, or prose approval is not the review judgment.
- Return without conclusion: report the result/limitations and leave the Ticket unconcluded. A finished turn, an artifact, or an approved MR does not itself decide completion.

These are instruction/tool contracts, not a claim that a mechanism proves real agent judgment. Backend authenticates actors and enforces typed boundaries; the responsible Worker/human must actually judge satisfaction against the natural-language request.

## Ticket tools and decisions

`QueryTicket` provides bounded discovery/filtering; `ShowTicket` provides authoritative item content and `content_digest`, paged thread, relations, Objectives, and any current MR context. Read the relevant Ticket before implementation, routing, review, state, or conclusion decisions. Check potential duplicates before creation or material rescope. Use `QueryObjective` and `ShowObjective` for broader context without replacing the Ticket read.

Useful mutation surfaces include `TicketCreate`, `TicketComment`, classification and relation/plan tools, `TicketOrchestratorAssign` (explicit delegation or reissue), `TicketOrchestratorClear`, fenced `TicketAssignmentAnswer`, `TicketAssignmentTrackingUpdate`, and `TicketCaseUpdate`. Read current tracking first; use `TicketAssignmentsQuery` for bounded retained rows and `TicketAssignmentDiagnostics` for sanitized separate input/start evidence. `QueryTicket` supports exact current principal and self-reported handling filters. Exposure depends on the effective Profile/Feature configuration and does not grant authority. Ready, Queue, six-state transitions and `CompleteTicket`/`TicketClose` are not current Worker operations; old events remain historical evidence.

Assign/reissue uses target principal, explicitly observed nullable `expected_assignment_id`, and stable `operation_id`; clear uses the observed assignment_id and operation_id. These role operations do not start/stop Workers. Answers use the same current assignment/event CAS and stable operation_key with bounded body, never an unfenced comment to manufacture continuation. Case judgments use current `content_digest` as `expected_content_digest`, expected open/closed state, explicitly observed nullable `closure_operation_key`, typed close/reopen/correct-closure action, explanation, stable operation key and optional references. Tracking updates use immutable `assignment_id`, observed `last_event_id` as `expected_last_event_id`, stable `operation_key` and the complete handling payload. Tag rename/delete mutations use stable tag identity and observed `expected_last_operation_key`; Ticket tag-set updates compare the observed full `expected_tag_ids` set and send the complete desired `tag_ids` set, never a display name or status-only comparison. Exact retry keeps the same key and entire input; changed input under that key is rejected. Replayed receipts are not fresh observations. After conflict, reread and reconcile, never silently overwrite or retry as a new operation against another assignment. Unknown dispatch outcomes are not success or failure. Saved Assign plus failed notification is not failed delegation and must not prompt another Assign. Do not infer missing delivery/acceptance diagnostics from Worker or handling state; report the concrete unavailable endpoint.

`TicketCaseUpdate` records an authenticated close, reopen or correction judgment with immutable reason/actor/history. It neither integrates an MR nor expands grants nor changes handling status. Handling done and case closed do not imply one another; done does not require unassignment. Reopen alone is not reissue or old Worker revival. Generic closure can occur without commits, MRs or approvals when consistent with the request, while explicit user approval conditions must be respected. Backend checks actor authority, CAS, replay and resource boundaries, not natural-language satisfaction. Code's guarded MR integration remains separate: do not claim an unmerged result was integrated.

Relations remain metadata, not a scheduling mechanism: record actual project dependencies, but do not encode capacity, Worker/worktree ownership, or execution ordering as relations. Use orchestration/session records for those concerns and inspect current Backend claim rules rather than inferring launch eligibility from a relation alone.

## Optional code Git/MR recipe

For code work using the default publication/review recipe, select `builtin:coder` and optionally `builtin:coder-review`. The Flow guides implement, review, fix, and handoff; it is not a Backend Ticket completion requirement and grants no authority. Read current repository state, use coherent commits and normal non-force publication only when authorized, validate the changed contracts, and report changed files, validation, and limitations. No commits/push should be inferred from a request explicitly prohibiting them.

Use one open repository-scoped MR per Ticket/repository. `OpenMergeRequest` creates selectors; once an MR exists, explicitly address its ID with `ShowMergeRequest` and preserve its selectors. Advance only the existing source selector with a normal non-force push. Do not invent an operation to append another source candidate, replacement MR, or fresh integration branch for every fix. Publish the exact committed source and verify provider resolution before requesting review.

The assigned Coder launches the Reviewer as an actual direct-child `builtin:reviewer` SubWorker, with explicit command grant and appropriate Workdir scope for inspection/validation, plus the structured Ticket/MR handoff. Server authority verifies assignment, Runtime-owned child identity, effective Profile, one-shot attempt, and captured subject. Review capability is injected by the trusted layer, not supplied by the model. The Reviewer makes an independent judgment against intent, acceptance criteria, the complete captured result snapshot, and validation, then records `ReviewMergeRequest`; prose and observation are not approval authority.

Source movement requires fresh review for the exact new source. Target-only movement preserves unchanged-source approval but requires refreshed integration evidence. Keep routine review/fix/rereview evidence on the MR rather than repeating each iteration in Ticket comments.

MR integration remains separate Orchestrator authority. Immediately before integration, reread Ticket/MR authority and `CheckMergeRequestReadiness`. Verify exact approved source, current target, strategy, and approval event. Apply and validate integration through ordinary source control in the bound Workdir, push normally, and verify provider resolution. `CompleteMergeRequest` records that already-applied repository result; it does not move a branch, complete the Ticket, or release assignment. If push succeeded but recording failed, reread/replay the same operation rather than pushing again. A Ticket request saying review is not required does not bypass MR integration guards.

## Responsibility, unfinished work, and cleanup (T-715)

Terminal current responsibility is retained for display after unfinished work ends. It is not the same as an unfinished assignment and does not require an invented unassignment operation before cleanup. A stopped or idle Worker may still own unfinished work; a completed/closed Ticket does not automatically stop/remove the Worker, release its attachments, or delete a Workdir. A result report or Flow terminal state is neither conclusion nor cleanup authority.

Worker stop, retention/removal, attachment release, and Workdir deletion are independent guarded decisions. Do not predeclare `delete_on_completion` or `retain_on_completion` at launch. Retain Workers while a requested review/fix or other handoff can still return; do not remove them just because one turn ended.

When authorized cleanup is in scope:

1. Reread Ticket responsibility, active unfinished work, Worker state, handoffs/notifications, and current retention constraints. Use the exact Worker subject from `WorkerList`.
2. Stop the Worker if needed and confirm terminal state. Stop alone does not end unfinished work. `UnfinishedWork` (`unfinished_work`) requires explicit authorized work end or reassignment and a fresh authority read.
3. Call `WorkerRemove` only with no unfinished work, running/restoring state, pin/legal hold, pending notification, or handoff. Backend revalidates guards; removal releases attachments but preserves Workdir materialization.
4. Reread actual Workdir attachment release, occupancy, cleanliness, ownership, provider availability, and other use. Retain still-needed/existing resources. Only then use `WorkdirDelete` for a proven clean, unoccupied, no-longer-needed Ticket-dedicated Workdir owned/selected for this work.

Never delete before attachment release, force removal, discard changes, or advance on a partial/unknown failure. Reread authority before a bounded retry. Cleanup failures do not roll back recorded Ticket or MR outcomes; report concrete blockers rather than adding routine success comments.

## Authoring and granularity

Intake clarifies ambiguous intent, checks duplicates and relevant context, and records agreed work. Create/update only after user agreement or explicit instruction to record the draft. A minimum investigation gate is an authoring responsibility, not a universal implementation approval gate. Intake is not scheduling, implementation, review, merge, or cleanup authority.

A useful Ticket records background/motivation, requirements, acceptance criteria, binding decisions/invariants, implementation latitude, and relevant open questions/risks. Include useful result evidence and final resolution when appropriate. Separate confirmed facts, user claims, hypotheses, and undecided questions. Do not copy Worker role boundaries, a temporary plan, or a decision to stop after authoring into the Ticket as a product prohibition. Record an approval gate only when the user asks for it; do not prematurely prescribe implementation tactics.

Risk flags prompt context checks and reviewer focus, not automatic stop gates. Record a waiting reason and next action only for a named missing decision/information after bounded checks; escalate beyond-scope decisions without inventing new constraints or moving the case to a planning state.

Do not create new umbrella/progress-container Tickets. Split broad work into concrete useful outcomes, record the split in Ticket/Objective context, and use Objectives for enduring medium-term goals. Do not replace umbrellas with parent/child, sub-ticket, part-of, contains, or other hierarchy relations. A concrete planning/design/investigation Ticket is valid when requested. Existing umbrellas can be retired as superseded/decomposed without rewriting history; distinguish container retirement from completion of future work.

Keep long research dumps outside the item body and summarize necessary bounded artifacts. Never store secrets, credentials, private prompt contents, or secret-bearing logs in Ticket/MR/Drive records or model-visible evidence.

## Configuration and validation

Workspace Ticket data and authority live in Server SQLite and active Workspace configuration. `yoi init --display-name <NAME> --repository-key <KEY>` registers a repository through Backend and writes global client routing only. Repository-local `.yoi/workspace.toml` and `.yoi/ticket.config.toml` do not grant Workspace identity, Backend connection, role selection, or Ticket authority. Legacy `ticket init` and `ticket import-local` are unsupported; no automatic migration reads local Ticket trees.

Dashboard actions are explicit user actions, not scheduler/auto-maintainer authority. Launch success without a visible result calls for attaching to or observing the Worker and rereading authority, not assuming conclusion. Select an accessible Workspace and current registered Profile when a launch reports missing configuration.

Validate the actual result with checks appropriate to its acceptance criteria. Code changes follow `AGENTS.md` and [Rust testing strategy](rust-testing-strategy.md): begin with the smallest contract-proving target/filter, then the required compile/test/format checks. Record only validation actually run. Narrow effective instruction/tool fixtures protect inclusion, schema, and policy wording; they do not prove real Worker judgment or end-to-end behavior.
