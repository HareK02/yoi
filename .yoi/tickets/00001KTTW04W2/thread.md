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
