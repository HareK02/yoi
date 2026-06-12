<!-- event: create author: "yoi ticket" at: 2026-06-11T08:15:24Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: intake at: 2026-06-11T10:21:40Z -->

## Intake summary

既存 Ticket 00001KTTW04W2 の body/thread/artifacts と関連 Ticket を確認した。これは implementation_ready。範囲は Orchestrator の progress を Companion に read-only weak notification として渡すことであり、AutoKick / re-kick / scheduler ではない。`Method::Notify` に `auto_run: bool` を追加し、Companion progress notice では `auto_run: false` を使う。`auto_run:false` は idle Pod を起こさず NotifyBuffer に積むだけで、live/reachable Companion への best-effort delivery に限定し、missing/stopped Companion の spawn/restore や初期 persistent snapshot は行わない。通知内容は durable/queryable state から bounded に生成し、history に残らない context-only injection、secret/unbounded log、Companion への mutation/spawn/merge authority 付与は禁止。関連の Companion lifecycle/profile policy は closed 済みで、この Ticket は starvation prevention Ticket 00001KTJXS31R とは非重複の follow-up。blocking open questions はない。risk_flags: [notification-semantics, panel-lifecycle, companion-policy, authority-boundary, prompt-context, persistence, sensitive-content]。

---

<!-- event: state_changed author: intake at: 2026-06-11T10:21:40Z from: planning to: ready reason: intake_ready field: state -->

## State changed

Intake refinement により、AutoKick/re-kick との差分、Companion authority 境界、weak notification semantics、bounded/safe context、validation focus が整理され、Orchestrator が routing できる状態になった。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-11T10:31:56Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-06-11T16:07:34Z -->

## Decision

Routing decision: implementation_ready（spawn は explicit follow-up まで保留）

Reason:
- Ticket body/thread は Orchestrator progress を Companion に read-only weak notification として渡す範囲を十分に固定している。
- AutoKick / re-kick / scheduler 化は非目標として明示されており、Companion authority 強化、missing/stopped Companion の spawn/restore、persistent snapshot 初期導入、context-only injection、secret/unbounded log 流入も禁止されている。
- 残る不確実性は既存 Notify / Panel / Companion 実装内での local tactic selection と targeted tests に閉じており、実装前に人間が追加で固定すべき product/API/authority decision は見つからない。
- 今回の launch instruction は「explicit follow-up before spawning role Pods」なので、ここでは queued -> inprogress、worktree 作成、coder/reviewer spawn は行わない。

Evidence checked:
- Ticket item/thread: Background、Requirements、Binding decisions / invariants、受け入れ条件、非目標、intake_summary、ready -> queued event。
- TicketRelationQuery: relation なし。unresolved depends_on / incoming blocks なし。
- TicketOrchestrationPlanQuery: 既存 plan なし。今回 accepted_plan を記録済み。
- TicketDoctor: error 0。
- repository state: `/home/hare/Projects/yoi` は dirty file なし、`develop...origin/develop [ahead 3]`。
- worktree/branch state: 既存 implementation worktree はなし。`ticket/orchestrator-progress-companion-notify` branch は存在するが `origin/develop` 相当で、実装開始時に merge target の現 HEAD との整合を確認する。
- bounded code map: `crates/protocol/src/lib.rs` の `Method::Notify`、`crates/pod/src/controller.rs` の Notify handling / RunForNotification、`crates/tui/src/multi_pod.rs` と workspace panel 周辺、`resources/profiles/companion.lua` / profile feature policy。

IntentPacket:

Intent:
- Orchestrator の Ticket 消化 progress を、live/reachable Companion に read-only weak notification として届け、Panel からその progress context の鮮度/last updated を確認できるようにする。

Binding decisions / invariants:
- `Notify { auto_run: false }` は idle Pod を起こさず、RunForNotification を staged しない。
- progress notice は AutoKick / re-kick / scheduler trigger にしない。
- Companion が missing/stopped の場合、通知だけで spawn/restore しない。
- 初期実装では persistent progress snapshot store を導入しない。
- Companion default profile の tool/feature policy を強化せず、Ticket mutation / Pod spawn / merge / worktree cleanup authority を与えない。
- Companion model context に渡す情報は history に残る形で扱い、history に残らない transient context-only injection をしない。
- 通知内容は durable/queryable state から bounded に生成し、secret/private context、sensitive provider error detail、unbounded logs、全 Ticket thread / Pod output を含めない。
- Prompt / workflow の LLM-facing framing を Rust code に直書きしない。必要なら `resources/prompts` 側に置く。

Requirements / acceptance criteria:
- live/reachable Companion が `Notify { auto_run: false }` 経由で Orchestrator progress notice を受け取れる。
- missing/stopped Companion では spawn/restore せず、best-effort delivery に留める。
- bounded summary generation と sensitive/unbounded content 排除が testable である。
- Panel から Companion progress context の鮮度または last updated が分かる。
- targeted tests を追加/更新する。

Implementation latitude:
- `auto_run` の serialization default / migration details、delivery helper の配置、Panel freshness 表示の具体的 UI placement、summary builder の内部構造、test の分割は既存設計に沿う範囲で coder が選んでよい。
- 既存 branch/worktree の扱いは実装開始時に Orchestrator が再確認し、merge target と整合する安全な branch/worktree で進める。

Escalate if:
- Companion に新しい mutation/spawn/merge authority を持たせる必要が出た場合。
- `auto_run:false` が history-backed notification 以外の hidden context injection を要求する場合。
- missing/stopped Companion 向け persistent snapshot store が初期実装の必須要件になりそうな場合。
- Method/Protocol の互換性・serde 形式で既存 session/log を壊す設計変更が必要になった場合。
- Progress notice が scheduler / AutoKick / re-kick の実行契機になりそうな場合。

Validation:
- targeted tests for `Method::Notify { auto_run: false }` idle behavior, live Companion delivery, missing/stopped no spawn/restore, bounded summary, sensitive/unbounded exclusion。
- `cargo test -p tui` または該当 targeted tests。
- `cargo fmt --check`。
- `git diff --check`。
- `/home/hare/Projects/yoi/target/debug/yoi ticket doctor`。
- runtime resource / prompt / packaging に触れた場合は `nix build .#yoi`。

Current code map:
- `crates/protocol/src/lib.rs`: `Method::Notify` schema / request handling type。
- `crates/pod/src/controller.rs`: `Notify` handling、NotifyBuffer、RunForNotification staging。
- `crates/tui/src/multi_pod.rs` / workspace panel 周辺: Companion delivery and freshness UI。
- `resources/profiles/companion.lua` and profile policy code: Companion authority remains read-only/limited。

Critical risks / reviewer focus:
- `auto_run:false` が idle Companion を起こしていないこと。
- progress notification が Orchestrator scheduler / AutoKick / re-kick と結合していないこと。
- Companion authority が増えていないこと。
- context は history-backed で、hidden transient injection になっていないこと。
- bounded/sensitive-safe summary が enforced/tested されていること。
- missing/stopped Companion で spawn/restore しないこと。

Next action:
- ユーザー/上位 Orchestrator から実装開始の explicit follow-up が来たら、side effect 前に TicketShow / relation / orchestration plan / git/worktree state を再確認し、問題なければ `queued -> inprogress` を記録してから worktree 作成と sibling coder/reviewer routing に進む。

---

<!-- event: decision author: orchestrator at: 2026-06-12T14:50:56Z -->

## Decision

Routing follow-up: implementation start authorized by user.

Recheck summary:
- Ticket remains `queued` and previously recorded `implementation_ready` still applies.
- TicketRelationQuery: no unresolved `depends_on` / incoming `blocks` for this Ticket.
- TicketOrchestrationPlan: accepted plan `orch-plan-20260611-160703-1` names branch/worktree and defers implementation until explicit follow-up; that follow-up has now arrived.
- Orchestrator worktree is clean.
- Visible active work: `00001KTVJFT6F` has coder `yoi-coder-panel-focus-model` running on a separate branch/worktree. There may be minor overlap in `crates/tui/src/multi_pod.rs` around Panel UI, so coder/reviewer must keep the Panel freshness change narrow and integration will recheck conflicts before merge.

Decision:
- Accept this Ticket now and proceed to `queued -> inprogress` before worktree/Pod side effects.
- Use existing accepted plan branch `ticket/orchestrator-progress-companion-notify` and worktree `/home/hare/Projects/yoi/.worktree/orchestrator-progress-companion-notify`.
- Continue to avoid root/original workspace operations; implementation side effects are limited to the child worktree and sibling Pods.

---

<!-- event: state_changed author: orchestrator at: 2026-06-12T14:51:03Z from: queued to: inprogress reason: orchestrator_acceptance field: state -->

## State changed

Explicit user follow-up authorized starting the previously accepted implementation plan. Ticket body/thread, relation blockers, accepted orchestration plan, current Orchestrator workspace state, and visible active work were rechecked. No unresolved blocker or missing planning decision remains. Implementation side effects will start only after this accepted `queued -> inprogress` transition is recorded.

---

<!-- event: plan author: orchestrator at: 2026-06-12T14:51:31Z -->

## Plan

Implementation worktree created for multi-agent handoff.

- Ticket: `00001KTTW04W2`
- Branch: `ticket/orchestrator-progress-companion-notify`
- Worktree: `/home/hare/Projects/yoi/.worktree/orchestrator-progress-companion-notify`
- Base: Orchestrator workspace HEAD `05fe1f6f` (`orchestration/yoi-orchestrator`), including the accepted routing record and `queued -> inprogress` transition.
- Concurrent work note: `00001KTVJFT6F` is active in `/home/hare/Projects/yoi/.worktree/panel-focus-composer-row-selection`; any Panel UI edits should be narrow and integration will recheck conflict risk.
- Scope rule: implementation work happens only in this child worktree. Root/original workspace must not be read, written, validated, merged, cleaned, or used for git operations.
- Next: spawn sibling coder with narrow write scope to the implementation worktree. Reviewer will be started after coder evidence is available.

---

<!-- event: plan author: orchestrator at: 2026-06-12T14:52:34Z -->

## Plan

Coder delegated.

- Coder Pod: `yoi-coder-companion-progress-notify`
- Worktree: `/home/hare/Projects/yoi/.worktree/orchestrator-progress-companion-notify`
- Branch: `ticket/orchestrator-progress-companion-notify`
- Scope: write `/home/hare/Projects/yoi/.worktree/orchestrator-progress-companion-notify`; read child worktree plus minimal non-recursive runtime-root read required by spawn validation.
- Task: implement `Notify { auto_run: false }` weak progress notification semantics, bounded Orchestrator progress summary delivery to live/reachable Companion, missing/stopped no spawn/restore behavior, Panel freshness/last-updated indication, focused tests, and required validation.
- Concurrent work note: `00001KTVJFT6F` has active coder work in another worktree and may edit Panel UI; coder was instructed to keep Panel changes narrow and report overlap if it becomes a blocker.
- Constraint: coder was instructed not to use root/original workspace for decision evidence or git/validation/cleanup, and not to create generated memory/local/runtime/log/lock/secret-like `.yoi` paths.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-12T15:23:02Z -->

## Implementation report

Coder completed implementation and branch was synchronized with current Orchestrator HEAD before review.

Coder Pod:
- `yoi-coder-companion-progress-notify`

Implementation branch/worktree:
- Branch: `ticket/orchestrator-progress-companion-notify`
- Worktree: `/home/hare/Projects/yoi/.worktree/orchestrator-progress-companion-notify`
- Implementation commit: `a87d3154 feat: weak companion progress notify`
- Sync merge commit on implementation branch: `724b79f1 Merge branch 'orchestration/yoi-orchestrator' into ticket/orchestrator-progress-companion-notify`
- Worktree status: clean

Changed surfaces:
- `crates/protocol/src/lib.rs`
- `crates/pod/src/controller.rs`
- `crates/pod/src/discovery.rs`
- `crates/pod/tests/controller_test.rs`
- `crates/tui/src/multi_pod.rs`

Implementation summary:
- Added `Method::Notify { auto_run: bool }`; missing `auto_run` defaults to `true`, `true` serializes like the old shape, and `false` is explicit.
- Idle Pod `Notify { auto_run: false }` does not stage `RunForNotification`; explicit next run drains notification history-backed through existing paths.
- Existing peer notify/discovery paths keep `auto_run: true`.
- Panel sends bounded progress notices to live/reachable Companion only via weak `Notify { auto_run: false }`.
- Missing/stopped/unreachable Companion is best-effort no-op; no spawn/restore.
- Progress summary is bounded to Ticket id/title/state, role pod status, and `.yoi/tickets/<id>` refs; no full thread, long output, diagnostics, provider errors, or secret-like content.
- Panel title shows Companion progress freshness/last-updated without adding a persistent snapshot store.
- Companion profile/tool authority is unchanged.

Orchestrator-verified evidence from child worktree after sync with current Orchestrator branch:
- `git status --short --branch`: clean on `ticket/orchestrator-progress-companion-notify`
- `cargo test -p protocol`: pass, 39 tests
- `cargo test -p pod --test controller_test`: pass, 36 tests
- `cargo test -p tui companion_progress -- --nocapture`: pass, 5 tests
- `cargo test -p tui send_notify_only_can_deliver_weak_notification_without_auto_run -- --nocapture`: pass, 1 test
- `cargo check -p protocol -p pod -p tui`: pass
- `cargo fmt --check`: pass
- `git diff --check orchestration/yoi-orchestrator...HEAD`: pass
- `cargo run -p yoi -- ticket doctor`: `doctor: ok`
- `nix build .#yoi`: pass

Known unrelated validation failures from coder full-suite notes:
- `cargo test -p pod` still fails existing prompt-resource assertion tests: `worker_instruction_override_from_resources`, `worker_instruction_uses_resource_template`.
- `cargo test -p tui` still fails existing tests: `multi_pod::tests::orchestrator_launch_context_includes_ticket_and_workspace_facts`, `multi_pod::tests::spawn_or_restore_companion_restores_live_pod`, `multi_pod::tests::spawn_or_restore_companion_spawns_missing_pod`.
- Focused weak-notify/progress tests pass; reviewer should confirm the broad failures are unrelated/pre-existing and not introduced by this Ticket.

Next:
- Spawn read-only sibling reviewer against the recorded intent, binding decisions/invariants, acceptance criteria, implementation commit `a87d3154`, synced review head `724b79f1`, diff, validation evidence, and known broad-suite failures.

---

<!-- event: plan author: orchestrator at: 2026-06-12T15:23:52Z -->

## Plan

Reviewer delegated.

- Reviewer Pod: `yoi-reviewer-companion-progress-notify`
- Target branch/worktree: `ticket/orchestrator-progress-companion-notify` at `/home/hare/Projects/yoi/.worktree/orchestrator-progress-companion-notify`
- Implementation commit: `a87d3154 feat: weak companion progress notify`
- Synced review head: `724b79f1 Merge branch 'orchestration/yoi-orchestrator' into ticket/orchestrator-progress-companion-notify`
- Scope: read-only child worktree plus minimal non-recursive runtime-root read required by spawn validation.
- Review basis: recorded IntentPacket, binding decisions/invariants, acceptance criteria, implementation summary, diff after syncing with current Orchestrator branch, focused validation evidence, `nix build` evidence, and known broad-suite failures.
- Reviewer focus: no AutoKick/re-kick/scheduler behavior, legacy notify compatibility, bounded/history-backed progress notices, missing/stopped Companion no-op, no authority expansion, Panel freshness UI compatibility with merged focus model, and robust focused tests.

---

<!-- event: review author: reviewer:yoi-reviewer-companion-progress-notify at: 2026-06-12T15:30:03Z status: request_changes -->

## Review: request changes

Request changes.

Blocker:
- `crates/tui/src/multi_pod.rs` の `companion_progress_notice()` が、Companion の model context に入る progress notice の framing 文言を Rust に直書きしている。
- この notice は `dispatch_companion_progress_notice()` から `Notify { auto_run: false }` として送られ、NotifyBuffer 経由で history-backed な SystemItem / LLM context になるため、単なる Panel UI 文言ではなく LLM-facing framing と判断する。
- Ticket の明示要件「Prompt / workflow 文言を Rust code に直書きしない」「LLM-facing summary framing が必要な場合は `resources/prompts` 側に置く」に反している。

Requested change:
- Rust 側は bounded runtime values の組み立てに留める。
- progress notice の framing/template は `resources/prompts` 配下へ移す。
- template/resource rendering の focused test を追加または既存 `companion_progress` tests を更新し、framing が resource 側から来ることと bounded/sensitive-safe values が維持されることを確認する。

Good evidence confirmed:
- `auto_run:false` は idle Pod に `RunForNotification` を staged しない。
- `auto_run:true` と legacy missing field の挙動は維持されている。
- live/reachable Companion の socket に `Notify { auto_run: false }` を送るだけで、missing/stopped/unreachable では spawn/restore されない。
- 通知内容は Ticket id/title/state、role pod status、short reason、`.yoi/tickets/<id>` ref に絞られ、thread excerpt / diagnostics / provider error token はテストで除外確認されている。
- Companion profile / authority 強化は diff 上ない。
- Panel freshness は header detail 表示で、composer target / row selection の focus model には干渉していない。

Reviewer-run validation:
- `git diff --check orchestration/yoi-orchestrator...HEAD`: pass
- `cargo test -p protocol`: pass, 39 tests
- `cargo test -p pod --test controller_test`: pass, 36 tests
- `cargo test -p tui companion_progress -- --nocapture`: pass, 5 tests
- `cargo test -p tui send_notify_only_can_deliver_weak_notification_without_auto_run -- --nocapture`: pass, 1 test
- `cargo check -p protocol -p pod -p tui`: pass
- `cargo fmt --check`: pass
- `cargo run -p yoi -- ticket doctor`: `doctor: ok`
- `nix build .#yoi`: pass

Broad-suite failures:
- `cargo test -p pod` currently fails 2 prompt-resource assertion tests.
- `cargo test -p tui` currently fails 3 unrelated-looking tests.
- These broad-suite failures do not appear caused by the progress-notify diff and are not the requested-change blocker.

---

<!-- event: plan author: orchestrator at: 2026-06-12T15:31:01Z -->

## Plan

Fix loop delegated to coder.

Reviewer blocker:
- `companion_progress_notice()` hard-coded LLM-facing progress notice framing in Rust.
- The notice reaches Companion model context through `Notify { auto_run: false }` / NotifyBuffer, so it must follow the prompt-resource boundary.

Coder fix request:
- Move progress notice framing/template to `resources/prompts` using existing resource conventions.
- Keep Rust limited to bounded runtime values and resource rendering/loading.
- Preserve weak notify semantics, live/reachable-only delivery, missing/stopped no spawn/restore, bounded/sensitive-safe summary, no persistent snapshot store, and no authority expansion.
- Add/update focused tests proving resource-backed framing and bounded/sensitive-safe runtime values.

Status:
- Sent fix request to `yoi-coder-companion-progress-notify` on existing branch/worktree.
- Reviewer verdict remains `request_changes` until coder fix and second review.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-12T15:40:14Z -->

## Implementation report

Coder completed requested fix.

Coder Pod:
- `yoi-coder-companion-progress-notify`

Implementation branch/worktree:
- Branch: `ticket/orchestrator-progress-companion-notify`
- Worktree: `/home/hare/Projects/yoi/.worktree/orchestrator-progress-companion-notify`
- Original implementation commit: `a87d3154 feat: weak companion progress notify`
- Sync merge commit: `724b79f1 Merge branch 'orchestration/yoi-orchestrator' into ticket/orchestrator-progress-companion-notify`
- Fix commit: `61e6c068 fix: resource-back companion progress notice`
- Worktree status: clean

Fix summary:
- Added `resources/prompts/panel/companion_progress_notice.md` for model-facing Companion progress notice framing.
- Removed hard-coded progress notice framing from Rust.
- Rust now builds bounded runtime values and renders the prompt resource via `minijinja`.
- Added focused test proving the notice renders from the prompt resource.
- Preserved live/reachable-only delivery, missing/stopped no-op, `auto_run:false`, bounded output, sensitive-field exclusion, and no persistent snapshot store.
- Added `minijinja` to `crates/tui/Cargo.toml`, updated `Cargo.lock`, and updated `package.nix` `cargoHash` for Nix packaging.

Orchestrator-verified evidence from child worktree after fix:
- `git status --short --branch`: clean on `ticket/orchestrator-progress-companion-notify`
- `cargo test -p protocol`: pass, 39 tests
- `cargo test -p pod --test controller_test`: pass, 36 tests
- `cargo test -p tui companion_progress -- --nocapture`: pass, 6 tests
- `cargo test -p tui send_notify_only_can_deliver_weak_notification_without_auto_run -- --nocapture`: pass, 1 test
- `cargo check -p protocol -p pod -p tui`: pass
- `cargo fmt --check`: pass
- `git diff --check orchestration/yoi-orchestrator...HEAD`: pass
- `cargo run -p yoi -- ticket doctor`: `doctor: ok`
- `nix build .#yoi`: pass

Next:
- Request reviewer re-review of fix commit `61e6c068`, focusing on the prompt-resource boundary blocker, retention of weak notify safety invariants, dependency/package updates, and validation evidence.

---

<!-- event: review author: reviewer:yoi-reviewer-companion-progress-notify at: 2026-06-12T15:42:21Z status: approve -->

## Review: approve

Approve.

The previous requested-change blocker is resolved.

Review findings:
- Prompt/resource boundary is now satisfied:
  - Companion progress notice framing lives in `resources/prompts/panel/companion_progress_notice.md`.
  - Rust only builds bounded runtime values and renders the prompt resource via `minijinja`.
  - `COMPANION_PROGRESS_NOTICE_TEMPLATE` uses `include_str!` to load the resource.
  - `render_companion_progress_notice_template()` renders with strict `minijinja` undefined behavior.
  - Focused test `companion_progress_notice_uses_prompt_resource_template` confirms rendering comes from the resource.
  - Grep for prior hard-coded framing found no Rust production copy of the prompt text; only test assertion remains.
- Weak notify invariants remain intact:
  - Companion progress dispatch still uses `send_notify_only(..., false)`.
  - `auto_run:false` controller behavior remains covered by pod controller tests.
  - `auto_run:true` legacy behavior remains covered by protocol/controller tests.
- Missing/stopped/unreachable Companion remains no-op:
  - Target selection still requires Companion panel state to be peer-reachable and a live reachable socket.
  - No spawn/restore path is introduced for progress delivery.
- No Companion authority expansion found:
  - The fix touches prompt rendering, TUI dependency metadata, `Cargo.lock`, and `package.nix`; it does not alter Companion profile/tool authority.
- Bounded/sensitive-safe summary remains preserved:
  - Runtime values are bounded before/after rendering.
  - The summary still uses ticket id/state/title/ref and role pod status, not thread excerpts, diagnostics, Pod output, or provider error details.
- Dependency/package updates are appropriate:
  - `minijinja = "2.19.0"` is already used elsewhere in the workspace (`crates/pod`), so adding it to `crates/tui` does not introduce an unexpected new dependency family.
  - `Cargo.lock` and `package.nix` `cargoHash` were updated, and `nix build .#yoi` passes.

Reviewer-run validation:
- `git diff --check orchestration/yoi-orchestrator...HEAD`: pass
- `cargo test -p tui companion_progress -- --nocapture`: pass, 6 tests
- `cargo test -p tui send_notify_only_can_deliver_weak_notification_without_auto_run -- --nocapture`: pass, 1 test
- `cargo check -p protocol -p pod -p tui`: pass
- `cargo fmt --check`: pass
- `cargo run -p yoi -- ticket doctor`: `doctor: ok`
- `nix build .#yoi`: pass
- `cargo test -p protocol`: pass, 39 tests
- `cargo test -p pod --test controller_test`: pass, 36 tests

No blocking issues remain.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-12T15:43:51Z -->

## Implementation report

Integration completed in the Orchestrator workspace.

Integrated implementation:
- Ticket: `00001KTTW04W2`
- Implementation branch: `ticket/orchestrator-progress-companion-notify`
- Implementation commits:
  - `a87d3154 feat: weak companion progress notify`
  - `61e6c068 fix: resource-back companion progress notice`
- Reviewer result: approve after fix-loop, no blockers remain
- Orchestrator merge commit: `56b10a2d merge: companion weak progress notify`
- Integration target: `orchestration/yoi-orchestrator`

Post-integration validation from Orchestrator worktree:
- `cargo test -p protocol`: pass, 39 tests
- `cargo test -p pod --test controller_test`: pass, 36 tests
- `cargo test -p tui companion_progress -- --nocapture`: pass, 6 tests
- `cargo test -p tui send_notify_only_can_deliver_weak_notification_without_auto_run -- --nocapture`: pass, 1 test
- `cargo check -p protocol -p pod -p tui`: pass
- `cargo fmt --check`: pass
- `git diff --check HEAD~1..HEAD`: pass
- `./result/bin/yoi ticket doctor`: `doctor: ok`
- `nix build .#yoi`: pass

Fix-loop resolution:
- Reviewer blocker about Rust-hard-coded LLM-facing progress notice framing was resolved by moving framing to `resources/prompts/panel/companion_progress_notice.md` and rendering it from bounded runtime values.

Remaining known broad-suite failures:
- Existing prompt/TUI broad-suite failures noted earlier remain outside this Ticket and were not blockers for the focused implementation/review.

Remaining risks:
- None blocking. Added `minijinja` dependency to `crates/tui`, already used elsewhere in workspace; `Cargo.lock` and `package.nix` were updated and Nix build passed.

---

<!-- event: state_changed author: orchestrator at: 2026-06-12T15:43:57Z from: inprogress to: done reason: implementation_merged_and_validated field: state -->

## State changed

Implementation was merged into the Orchestrator branch after reviewer-requested fix, reviewer approved with no blockers, and post-integration validation passed. Cleanup and closure will be recorded next.

---
