<!-- event: create author: LocalTicketBackend at: 2026-06-11T16:03:28Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: intake at: 2026-06-11T16:03:33Z -->

## Intake summary

Ticket role launch の初回 user message を短縮し、情報所管を Instruction / Workflow / Submit / control-plane に分離する方針で合意済み。language と workspace_root/cwd は Instruction 側の環境情報、Git/worktree 操作対象として必要な path は Submit に重複記載可。workflow slug/profile selector は初回 user message に含めない。

---

<!-- event: state_changed author: intake at: 2026-06-11T16:03:33Z from: planning to: ready reason: planning_ready field: state -->

## State changed

要件・受け入れ条件・境界判断が揃ったため、Orchestrator routing 可能。実装はユーザーが panel で queue した後に開始する。

---

<!-- event: decision author: orchestrator at: 2026-06-12T09:11:30Z -->

## Decision

Routing decision: blocked_by_dependency_or_missing_authority

Reason:
- Workspace Panel Queue notification was received, but this Orchestrator backend still reads the Ticket as `ready`, not `queued`.
- The root workspace has unsynced/uncommitted queue-related changes for this Ticket and is dirty, including `.yoi/tickets/00001KTVPS6K3/*` and `crates/tui/src/multi_pod.rs`.
- Active in-progress work `00001KTWPE3KQ` is currently fixing the Panel Queue durable handoff/sync path and is in reviewer handoff; accepting this new Ticket before that path is reviewed/merged would require manual sync/queue recovery and could mix queue-side effects with unrelated dirty root changes.

Evidence checked:
- TicketShow `00001KTVPS6K3`: Orchestrator backend state is `ready`.
- TicketRelationQuery: no relation blockers.
- TicketOrchestrationPlanQuery: no prior plan records before this routing note.
- Root/orchestrator git state: Orchestrator branch has local routing record changes; root workspace is dirty with this Ticket's `.yoi` files and `crates/tui/src/multi_pod.rs`.
- Visible Pods: active reviewer `yoi-reviewer-panel-queue-sync` for `00001KTWPE3KQ`.

Next action:
- Leave this Ticket unaccepted for implementation in this Orchestrator pass.
- Re-route after the Panel Queue durable handoff work is resolved and the root/orchestration Ticket state is synchronized cleanly, or after a human explicitly instructs manual recovery for the queued root-side changes.

Escalate if:
- The queued root-side changes should be manually committed/synced despite the current dirty workspace and active Queue-handoff fix.

---

<!-- event: state_changed author: workspace-panel at: 2026-06-12T12:29:10Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-06-12T12:32:06Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Ticket body は Ticket role launch の first-run user message から role behavior / procedural guidance / control-plane metadata を外し、Submit に対象 Ticket / action instruction / per-launch context だけを残す方針を明確に記録している。
- Instruction / Workflow / Submit / control-plane の境界、Ticket config schema に instruction field を追加しないこと、launch context template 化を避けることが binding decision として記録済みである。
- `ready -> queued` は workspace-panel により再実行され、現在の Orchestrator backend でも `queued` として読める。relation blocker はない。
- 以前の blocker だった Panel Queue durable handoff work `00001KTWPE3KQ` は closed 済みで、現在の orchestration worktree は clean。残る不確実性は既存 prompt/profile/workflow resource と `crates/client/src/ticket_role.rs` 周辺の bounded implementation investigation に閉じている。

Evidence checked:
- Ticket body / thread / artifacts: `item.md`、`thread.md`、既存 waiting-capacity note を確認。
- TicketRelationQuery: outgoing/incoming relation なし、blocker なし。
- TicketOrchestrationPlanQuery: 既存 `waiting_capacity_note` と、今回の accepted plan `orch-plan-20260612-123134-2` を確認。
- Related Ticket: `00001KTWPE3KQ` は closed 済みで、Queue handoff blocker は解消済み。
- Workspace state: Orchestrator worktree `orchestration/yoi-orchestrator` は clean。
- Code/resource map: `crates/client/src/ticket_role.rs`, `resources/profiles/orchestrator.lua`, `resources/prompts/ticket_role/*.md` と関連 prompt/profile/workflow resource を確認。
- Durable context: prompt prose は resources/prompts へ集約し、Profile/Workflow が role behavior/procedure を所有する既存方針に一致する。
- Visible Pods: active child Pod なし。

IntentPacket:

Intent:
- Ticket role Pod launch の初回 user message を短縮し、model-visible な launch input を Submit-only の per-launch context に近づける。
- role behavior は builtin role Profile が選ぶ Instruction、procedural guidance は Workflow、control-plane metadata は launch plan/diagnostics/trace に分離する。

Binding decisions / invariants:
- `Profile selector`、`Workflow`、`Configured launch_prompt ref` などの control-plane metadata は first-run user message に含めない。
- role behavior / 恒常的環境情報 / language / workspace_root / cwd は Instruction/system 側の責務とする。ただし今回の Git/worktree 操作対象として必要な path は Submit に明示してよい。
- procedural guidance は Workflow 側に置く。
- Submit に残す情報は対象 Ticket、action instruction、今回の操作対象として必要な per-launch context に限定する。
- Ticket config に role-level `system_instruction` / `instruction` field を追加しない。
- launch context の過剰なテンプレート化は行わず、Rust 側の短い Submit 組み立てに留める。
- 新しい input を hidden context として差し込まず、必要な model-visible context は適切な Instruction/Workflow/committed user task/history 経由にする。

Requirements / acceptance criteria:
- Ticket role launch の first-run `Segment::Text` が短くなり、対象 Ticket / action instruction / per-launch context だけを含む。
- role behavior は builtin role instruction prompt から供給される。
- procedural guidance は Workflow に残る、または Workflow に移されている。
- control-plane metadata は prompt text ではなく launch plan / diagnostics / trace に留まる。
- Ticket record language guidance は first-run user message ではなく Instruction 側の環境情報として扱われる。
- Orchestrator launch で必要な workspace/cwd/topology 情報は Pod 環境情報と per-operation Submit 情報に分離され、Git/worktree 操作対象として必要なものだけ Submit 内で明示される。
- 関連 unit tests が新しい出力方針を検証する。
- `nix build .#yoi` が通る。

Implementation latitude:
- builtin role instruction prompt のファイル名・配置は既存 prompt resource conventions に合わせて選んでよい。
- `resources/prompts/ticket_role` の既存 fragment は、不要なら削除または縮小してよい。
- `TicketRoleLaunchContext.user_instruction` rename は、変更量が大きい場合は段階的移行でもよい。ただし model-visible label は `User/action instruction` より正確な名前にする。
- Workflow 側に移す文言は public builtin workflow と workspace dogfood workflow の責務差を保つ。

Escalate if:
- Instruction / Workflow / Submit の境界に収まらない新しい model-visible 情報が見つかる。
- Ticket role launch の dynamic data を Instruction minijinja context に入れる必要が出る。
- Ticket config schema に新しい instruction field を追加する必要が出る。
- Workflow invocation、workspace/user prompt override、Ticket role launch、Panel Intake / Orchestrator launch のいずれかを壊さないと実現できない。

Validation:
- Ticket role launch prompt 出力の unit tests。
- builtin role Profile が instruction prompt を解決できることの focused validation。
- relevant cargo tests。
- `cargo fmt --check`。
- `git diff --check`。
- `target/debug/yoi ticket doctor` または built binary の `yoi ticket doctor`。
- `nix build .#yoi`。

Current code map:
- 主対象: `crates/client/src/ticket_role.rs`。
- Profile/resource: `resources/profiles/{intake,orchestrator,coder,reviewer}.lua`, `resources/prompts/ticket_role/*.md`, 必要な `resources/workflows/*` / `.yoi/workflow/*`。
- Prompt/system integration check: `crates/pod/src/prompt/system.rs`。
- Implementation worktree では Orchestrator/main `.yoi` records、`.yoi/memory`、local/runtime/log/lock/secret-like `.yoi` paths を生成・編集しない。

Critical risks / reviewer focus:
- control-plane metadata が first-run user message に残っていないこと。
- role behavior / procedure が Submit に残留せず、Instruction / Workflow の適切な boundary に移っていること。
- Ticket record language guidance と workspace/cwd/topology 情報が hidden/ephemeral context injection になっていないこと。
- Dynamic Ticket data を system Instruction 側へ混ぜないこと。
- 既存 Ticket role launch、Panel Intake/Orchestrator launch、workflow invocation、prompt override の動作を壊していないこと。

Next action:
- `queued -> inprogress` を記録してから、branch `ticket/shorten-ticket-role-launch-input` / worktree `/home/hare/Projects/yoi/.worktree/shorten-ticket-role-launch-input` を作成し、sibling coder に narrow write scope で実装を委譲する。Reviewer は coder evidence 後に read-only で起動する。

---

<!-- event: state_changed author: orchestrator at: 2026-06-12T12:32:11Z from: queued to: inprogress reason: orchestrator_acceptance field: state -->

## State changed

Routing accepted for implementation. Ticket body/thread, relation blockers, orchestration plan, prior waiting-capacity note, closed Queue-handoff dependency, current Orchestrator workspace state, relevant prompt/profile/workflow code map, and visible Pods were rechecked. No unresolved blocker or missing planning decision remains. Implementation side effects will start only after this accepted `queued -> inprogress` transition is recorded, using accepted plan `orch-plan-20260612-123134-2`.

---

<!-- event: plan author: orchestrator at: 2026-06-12T12:32:36Z -->

## Plan

Implementation worktree created for multi-agent handoff.

- Ticket: `00001KTVPS6K3`
- Branch: `ticket/shorten-ticket-role-launch-input`
- Worktree: `/home/hare/Projects/yoi/.worktree/shorten-ticket-role-launch-input`
- Base: Orchestrator workspace HEAD `ae66c447` (`orchestration/yoi-orchestrator`), including the accepted routing record and `queued -> inprogress` transition.
- Scope rule: implementation work happens only in this child worktree. Root/original workspace must not be read, written, validated, merged, cleaned, or used for git operations.
- Next: spawn sibling coder with narrow write scope to the implementation worktree. Reviewer will be started after coder evidence is available.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-12T12:33:47Z -->

## Implementation report

Coder spawn initial attempt failed before socket creation.

- Coder Pod name: `yoi-coder-role-launch-input`
- Error: `cwd is not readable under the configured scope: /home/hare/Projects/yoi`
- Recovery: retry with the same child worktree write scope plus minimal non-recursive read authority on the runtime workspace root required by spawn validation. The coder task still explicitly prohibits reading/writing/validating/merging/cleaning/running git in the root/original workspace and must operate from the child worktree only.

---

<!-- event: plan author: orchestrator at: 2026-06-12T12:34:21Z -->

## Plan

Coder delegated.

- Coder Pod: `yoi-coder-role-launch-input`
- Worktree: `/home/hare/Projects/yoi/.worktree/shorten-ticket-role-launch-input`
- Branch: `ticket/shorten-ticket-role-launch-input`
- Scope: write `/home/hare/Projects/yoi/.worktree/shorten-ticket-role-launch-input`; read child worktree plus minimal non-recursive runtime-root read required by spawn validation.
- Task: shorten Ticket role launch first-run Submit input, move/keep role behavior in builtin Profile instruction, keep procedural guidance in Workflow, keep control-plane metadata out of first-run user text, add focused tests, validate with relevant cargo tests and `nix build .#yoi`.
- Constraint: coder was instructed not to use root/original workspace for decision evidence or git/validation/cleanup, and not to create generated memory/local/runtime/log/lock/secret-like `.yoi` paths.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-12T12:56:48Z -->

## Implementation report

Coder completed implementation.

Coder Pod:
- `yoi-coder-role-launch-input`

Implementation branch/worktree:
- Branch: `ticket/shorten-ticket-role-launch-input`
- Worktree: `/home/hare/Projects/yoi/.worktree/shorten-ticket-role-launch-input`
- Commit: `949531a0 client: shorten ticket role launch input`
- Worktree status: clean

Changed surfaces:
- `crates/client/src/ticket_role.rs`
- `crates/manifest/src/profile.rs`
- `crates/pod/src/prompt/loader.rs`
- `resources/profiles/{intake,orchestrator,coder,reviewer}.lua`
- `resources/prompts/role/{intake,orchestrator,coder,reviewer}.md`
- removed obsolete `resources/prompts/ticket_role/*.md`
- `resources/workflows/{multi-agent-workflow,ticket-intake-workflow,ticket-orchestrator-routing}.md`

Orchestrator-verified evidence from child worktree:
- `git status --short --branch` clean on `ticket/shorten-ticket-role-launch-input`
- `cargo test -p client ticket_role --lib`: pass, 16 tests
- `cargo test -p manifest builtin_role_profiles_are_registered_and_resolve --lib`: pass, 1 test
- `cargo test -p pod builtin_ticket_role_instructions_resolve --lib`: pass, 1 test
- `cargo fmt --check`: pass
- `git diff --check orchestration/yoi-orchestrator...HEAD`: pass
- `./result/bin/yoi ticket doctor`: `doctor: ok`
- `nix build .#yoi`: pass

Next:
- Spawn read-only sibling reviewer against the recorded intent, binding decisions/invariants, acceptance criteria, commit `949531a0`, diff, and validation evidence.

---

<!-- event: plan author: orchestrator at: 2026-06-12T12:57:29Z -->

## Plan

Reviewer delegated.

- Reviewer Pod: `yoi-reviewer-role-launch-input`
- Target branch/worktree: `ticket/shorten-ticket-role-launch-input` at `/home/hare/Projects/yoi/.worktree/shorten-ticket-role-launch-input`
- Commit under review: `949531a0 client: shorten ticket role launch input`
- Scope: read-only child worktree plus minimal non-recursive runtime-root read required by spawn validation.
- Review basis: recorded IntentPacket, binding decisions/invariants, acceptance criteria, implementation summary, diff, and Orchestrator-verified validation evidence.
- Reviewer focus: control-plane metadata absence from first-run user text, Instruction/Workflow/Submit boundary, no hidden dynamic context injection, profile instruction resolution, workflow/prompt override safety, and non-brittle tests.

---

<!-- event: review author: reviewer:yoi-reviewer-role-launch-input at: 2026-06-12T13:06:54Z status: approve -->

## Review: approve

Approve.

Review target:
- Worktree: `/home/hare/Projects/yoi/.worktree/shorten-ticket-role-launch-input`
- Commit: `949531a07ede4fd9f82787454ac4d996f25b272d`
- Worktree status: clean

Evidence:
- `crates/client/src/ticket_role.rs` の first-run `Segment::Text` は `build_launch_prompt` で対象 Ticket、`Action instruction`、Panel handoff、intent packet、operation/worktree target、validation/report expectations に限定されている。
- `profile` / `workflow` / `launch_prompt_ref` / workspace-root 系の制御情報は `TicketRoleLaunchPlan` / `SpawnConfig` 側に残り、Submit text には出ていない。
- `WorkflowInvoke` は `Segment::WorkflowInvoke` として別 segment に維持されている。
- unit tests は `Configured launch_prompt`、`Profile selector`、`Workflow:`、Ticket record language、`role_workspace_root` / `role_cwd` 等が first-run text に入らないことと、必要な path/branch/validation/report expectation が残ることを直接検証している。
- builtin role Profiles は `worker.instruction = "$yoi/role/<role>"` を設定し、`resources/prompts/role/*` に role behavior が移っている。procedural guidance は `resources/workflows/*` に残っている。
- 削除された `resources/prompts/ticket_role/*` への production code 参照は残っていない。prompt loader の `$user` / `$workspace` 解決機構と workflow invocation は維持されている。
- `git diff --check orchestration/yoi-orchestrator...HEAD` は問題なし。Orchestrator が提示した focused cargo tests、`cargo fmt --check`、`ticket doctor`、`nix build .#yoi` の validation evidence は受け入れ可能。

Non-blocking risk:
- 旧 `.yoi/prompts/ticket_role/*` override は、この surface の削除により launch Submit text には効かなくなる。今回の Ticket では既存 fragment の削除が許容されているため blocker ではないが、外部ユーザー向けには role behavior は Profile instruction、手順は Workflow override へ移す旨を後で案内すると安全。

---

<!-- event: implementation_report author: orchestrator at: 2026-06-12T13:08:07Z -->

## Implementation report

Integration completed in the Orchestrator workspace.

Integrated implementation:
- Ticket: `00001KTVPS6K3`
- Implementation branch: `ticket/shorten-ticket-role-launch-input`
- Implementation commit: `949531a0 client: shorten ticket role launch input`
- Reviewer result: approve, no blockers
- Orchestrator merge commit: `b18b8b5d merge: ticket role launch input split`
- Integration target: `orchestration/yoi-orchestrator`

Post-integration validation from Orchestrator worktree:
- `cargo test -p client ticket_role --lib`: pass, 16 tests
- `cargo test -p manifest builtin_role_profiles_are_registered_and_resolve --lib`: pass, 1 test
- `cargo test -p pod builtin_ticket_role_instructions_resolve --lib`: pass, 1 test
- `cargo fmt --check`: pass
- `git diff --check HEAD~1..HEAD`: pass
- `nix build .#yoi`: pass
- `./result/bin/yoi ticket doctor`: `doctor: ok`

Remaining non-blocking risk:
- 旧 `.yoi/prompts/ticket_role/*` override は launch Submit text には効かなくなる。今回の Ticket では fragment 削除が許容されているため blocker ではないが、外部ユーザー向けには role behavior は Profile instruction、手順は Workflow override へ移す旨を後続で案内すると安全。

---

<!-- event: state_changed author: orchestrator at: 2026-06-12T13:08:11Z from: inprogress to: done reason: implementation_merged_and_validated field: state -->

## State changed

Implementation was merged into the Orchestrator branch, reviewer approved, and post-integration validation passed. Cleanup and closure will be recorded next.

---
