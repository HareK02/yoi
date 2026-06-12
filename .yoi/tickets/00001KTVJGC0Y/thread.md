<!-- event: create author: LocalTicketBackend at: 2026-06-11T14:48:44Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: intake at: 2026-06-11T14:48:50Z -->

## Intake summary

`ticket.language` guidance が Ticket role launch prompt に偏っており、Companion など non-Ticket-role Pod が Ticket tools を使う場合に durable Ticket record / Ticket tool body の言語指示が届かない問題を concrete implementation Ticket として整理した。要件は、Ticket role に依存せず universal Ticket capability / tool surface または feature-scoped system prompt path から model-visible guidance を届けること。

---

<!-- event: state_changed author: intake at: 2026-06-11T14:48:50Z from: planning to: ready reason: planning_ready field: state -->

## State changed

要件・受け入れ条件・binding decisions・実装余地・escalation conditions・validation が揃っているため、Orchestrator routing 可能な ready Ticket とする。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-12T14:49:39Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-06-12T14:54:15Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Ticket body は `ticket.language` guidance を Ticket role launch prompt 専用ではなく、Ticket tools を持つすべての Pod に model-visible に届ける要件を明確に記録している。
- `worker.language` / `memory.language` / `ticket.language` の責務分離、hidden context-only injection 禁止、既存 Ticket record の一括 rewrite 禁止が binding invariant として記録済みである。
- risk flags は prompt-context / tool-description / feature-boundary / ticket-language / companion だが、bounded context check の結果、具体的な未決定 design/API/authority 判断は残っていない。実装方式は Ticket capability/tool surface または feature-scoped system prompt path の範囲で coder が選べる。
- Relation blocker はなく、OrchestrationPlan に accepted plan `orch-plan-20260612-145344-1` を記録済み。
- 現在 active coder は `00001KTVJFT6F`（Panel focus）と `00001KTTW04W2`（Companion progress notify）だが、この Ticket の主対象は Ticket language guidance の prompt/tool/feature boundary であり、Panel UI 変更とは独立している。Companion-adjacent確認はあるが、authority強化や progress notify implementation と結合しないように実装・reviewで確認する。

Evidence checked:
- Ticket body / thread: requirements, acceptance criteria, binding decisions, implementation latitude, escalation conditions, validation, intake summary, `ready -> queued` event を確認。
- TicketRelationQuery: outgoing/incoming relation なし、blocker なし。
- TicketOrchestrationPlanQuery: 既存 record なし、今回 accepted plan を記録。
- Code/resource map: `ticket.language` / `Ticket record language` / Ticket tool/backend/feature/prompt surfaces を narrow search で確認。直近の role launch split により Ticket role first-run prompt から language guidance が外れているため、本 Ticket の universal delivery 実装が次の自然な境界であることを確認。
- Workspace/Pod state: Orchestrator worktree clean、active child worktree は別 branch/scope。

IntentPacket:

Intent:
- `ticket.language` guidance を Ticket role 固有の launch prompt ではなく、Ticket-writing tools を持つすべての Pod に durable/model-visible な形で届ける。
- Companion-style non-Ticket-role context でも Ticket tool body / durable Ticket record を configured `ticket.language` に従って書く guidance が見えるようにする。

Binding decisions / invariants:
- `ticket.language` は durable Ticket record / Ticket tool body writing policy であり、Ticket role 固有 policy ではない。
- `worker.language` は通常 prose、`memory.language` は Memory/Knowledge generation、`ticket.language` は durable Ticket records / Ticket tool bodies の責務に分ける。
- `ticket.language` が設定されていても protocol literals、file paths、commands、logs、identifiers、quoted external text を不要に翻訳しない。
- hidden context-only injection を作らない。guidance は tool surface / feature-scoped system prompt / committed prompt path など、モデルから見える根拠を残す。
- 既存 Ticket records を翻訳・一括 rewrite しない。
- Companion default authority を強化しない。

Requirements / acceptance criteria:
- Ticket tools を持つ non-Ticket-role Pod、特に Companion-style context でも guidance が model-visible になる。
- Ticket role Pods でも同等 guidance が届き、既存挙動が退行しない。
- guidance source は universal Ticket capability / tool surface または feature-scoped system prompt path にあり、Ticket role launch prompt 専用ではない。
- `worker.language` が `ticket.language` を override しない。
- focused test または snapshot-style verification で Ticket role と generic / Companion-style Ticket-capable context の両方を確認する。
- `nix build .#yoi` が通る。

Implementation latitude:
- Ticket tool descriptions/schema text に configured Ticket language instruction を入れる方式、または Ticket feature/capability が有効な Pod への feature-scoped prompt guidance を使う方式を選んでよい。
- 既存 architecture を広く作り替えず、現在の ToolRegistry/feature/prompt resource 境界に合う最小実装を選ぶ。
- Tests は prompt snapshot 全体に brittle にせず、language guidance の存在/非混同を直接確認する。

Escalate if:
- Tool descriptions が configured Ticket language に依存できず、広い ToolRegistry redesign が必要になる。
- feature-scoped prompt guidance が history に残らない context mutation を必要とする。
- Companion の Ticket capability path から Ticket config にアクセスできない。
- 実装が Ticket record language と worker response language を混同する。
- Companion authority を増やす必要が出る。

Validation:
- Ticket role prompt/context の focused test。
- generic / Companion-style Ticket-capable context の guidance 確認 test。
- relevant focused `cargo test`。
- `cargo fmt --check`。
- `git diff --check`。
- `./result/bin/yoi ticket doctor` または同等。
- `nix build .#yoi`。

Current code map:
- `crates/ticket/src/config.rs`: `ticket.language` config parsing / validation。
- `crates/ticket/src/tool.rs`: Ticket tool definitions/descriptions and tests。
- `crates/pod/src/feature/builtin/ticket.rs`: Ticket feature registration/capability/tool exposure。
- `crates/pod/src/prompt/*`: feature/system prompt integration if needed。
- `crates/client/src/ticket_role.rs`: ensure Ticket role launch still receives guidance through the new universal path, not first-run Submit prose。

Critical risks / reviewer focus:
- guidance が Ticket role launch prompt 専用に戻っていないこと。
- non-role Ticket-capable context にも model-visible に届くこと。
- hidden ephemeral context injection ではなく、tool/feature/prompt surface として履歴・prompt根拠が残ること。
- `worker.language` と `ticket.language` を混同しないこと。
- Companion authority / Ticket mutation authority を強化しないこと。

Next action:
- `queued -> inprogress` を記録してから、branch `ticket/ticket-language-guidance-all-tools` / worktree `/home/hare/Projects/yoi/.worktree/ticket-language-guidance-all-tools` を作成し、sibling coder に narrow write scope で実装を委譲する。Reviewer は coder evidence 後に read-only で起動する。

---

<!-- event: state_changed author: orchestrator at: 2026-06-12T14:54:22Z from: queued to: inprogress reason: orchestrator_acceptance field: state -->

## State changed

Routing accepted for implementation. Ticket body/thread, relation blockers, orchestration plan, relevant Ticket language / tool / feature / prompt code map, current Orchestrator workspace state, and active parallel work were rechecked. No unresolved blocker or missing planning decision remains. Implementation side effects will start only after this accepted `queued -> inprogress` transition is recorded, using accepted plan `orch-plan-20260612-145344-1`.

---

<!-- event: plan author: orchestrator at: 2026-06-12T14:54:49Z -->

## Plan

Implementation worktree created for multi-agent handoff.

- Ticket: `00001KTVJGC0Y`
- Branch: `ticket/ticket-language-guidance-all-tools`
- Worktree: `/home/hare/Projects/yoi/.worktree/ticket-language-guidance-all-tools`
- Base: Orchestrator workspace HEAD `76d358e8` (`orchestration/yoi-orchestrator`), including the accepted routing record and `queued -> inprogress` transition.
- Parallel work note: active child worktrees are `panel-focus-composer-row-selection` and `orchestrator-progress-companion-notify`; this Ticket should stay within Ticket language/tool/feature/prompt guidance surfaces and avoid coupling to their implementation branches.
- Scope rule: implementation work happens only in this child worktree. Root/original workspace must not be read, written, validated, merged, cleaned, or used for git operations.
- Next: spawn sibling coder with narrow write scope to the implementation worktree. Reviewer will be started after coder evidence is available.

---

<!-- event: plan author: orchestrator at: 2026-06-12T14:55:42Z -->

## Plan

Coder delegated.

- Coder Pod: `yoi-coder-ticket-language-guidance`
- Worktree: `/home/hare/Projects/yoi/.worktree/ticket-language-guidance-all-tools`
- Branch: `ticket/ticket-language-guidance-all-tools`
- Scope: write `/home/hare/Projects/yoi/.worktree/ticket-language-guidance-all-tools`; read child worktree plus minimal non-recursive runtime-root read required by spawn validation.
- Task: make configured `ticket.language` guidance model-visible for all Ticket-capable Pods, including non-role Companion-style contexts; preserve worker/memory/ticket language boundaries; avoid hidden context-only injection; add focused tests and required validation.
- Parallel work note: active sibling worktrees for Panel focus and Companion progress notification exist; coder was instructed to avoid coupling and report concrete merge/design conflict if found.
- Constraint: coder was instructed not to use root/original workspace for decision evidence or git/validation/cleanup, and not to create generated memory/local/runtime/log/lock/secret-like `.yoi` paths.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-12T15:10:38Z -->

## Implementation report

Coder completed implementation.

Coder Pod:
- `yoi-coder-ticket-language-guidance`

Implementation branch/worktree:
- Branch: `ticket/ticket-language-guidance-all-tools`
- Worktree: `/home/hare/Projects/yoi/.worktree/ticket-language-guidance-all-tools`
- Commit: `92c4dee7 ticket: guide Ticket tool language universally`
- Worktree status: clean

Changed surfaces:
- `crates/ticket/src/tool.rs`
- `crates/pod/src/feature/builtin/ticket.rs`

Implementation summary:
- Ticket tool `ToolMeta.description` now derives from configured `LocalTicketBackend::record_language()`.
- Added shared `ticket_tool_description(name, record_language)` so all Ticket tool descriptions can include configured Ticket record language guidance.
- Guidance distinguishes durable Ticket record / Ticket tool body text from normal `worker.language` prose and `memory.language` Memory/Knowledge generation.
- Guidance preserves protocol literals, file paths, commands, logs, identifiers, and quoted external text when fidelity matters.
- Feature descriptor `ToolDeclaration` descriptions use the same helper, so Ticket role launch is not the only guidance path.
- Focused tests cover configured-language presence, unset-language omission, read-only Companion-style context, and lifecycle/Ticket-role-style context.
- Companion/read-only authority was not changed.

Orchestrator-verified evidence from child worktree:
- `git status --short --branch`: clean on `ticket/ticket-language-guidance-all-tools`
- `cargo test -p ticket ticket_record_language_guidance`: pass
- `cargo test -p pod ticket_language_guidance`: pass
- `cargo test -p ticket`: pass, 68 tests
- `cargo fmt --check`: pass
- `git diff --check`: pass
- `cargo run -p yoi -- ticket doctor`: `doctor: ok`
- `nix build .#yoi`: pass

Known unrelated validation failure:
- `cargo test -p pod` fails two prompt assertion tests:
  - `prompt::catalog::tests::pod_orchestration_guidance_section_renders_resource_body`
  - `prompt::system::tests::pod_orchestration_guidance_is_included_for_pod_management_tools`
- The same two focused tests fail on current Orchestrator branch before this implementation is merged, with the same missing phrase `worktree status, diff, and test results`.
- These failures are in Pod orchestration guidance prompt assertions, outside the Ticket tool language guidance change, but reviewer should confirm they are unrelated and not introduced by this Ticket.

Next:
- Spawn read-only sibling reviewer against the recorded intent, binding decisions/invariants, acceptance criteria, commit `92c4dee7`, diff, validation evidence, and known pre-existing unrelated `cargo test -p pod` failures.

---
