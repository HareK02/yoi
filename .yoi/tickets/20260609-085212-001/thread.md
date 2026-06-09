<!-- event: create author: LocalTicketBackend at: 2026-06-09T08:52:12Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: plan author: intake at: 2026-06-09T09:55:30Z -->

## Plan

## Intake refinement

readiness は `implementation_ready`。目的、対象 surface、非目標、受け入れ条件、検証観点がすでに具体的で、Orchestrator が routing できる。

### Binding decisions / invariants

- `TicketList` は selection / triage / backlog overview 用の bounded summary として扱い、routing / close / planning return / implementation acceptance の authority にはしない。
- 詳細判断の authority は `TicketShow <id>` の body/thread/artifacts とする。
- LLM-facing `TicketList` tool の default result は軽量・bounded にし、item body / thread / artifacts の詳細を漏らさない。
- CLI human output も default では巨大な JSON/Markdown を出さず、人間可読性と context safety を両立する。
- `state=all` や closed を含む一覧は特に context blow-up を起こしにくい default/max limit を持つ。
- long title / diagnostics / attention hint は bounded に切り詰める。
- Ticket backend schema、TicketShow の詳細性、Ticket relation / Objective / OrchestrationPlan の設計はこの Ticket の範囲外。

### Implementation latitude

- 1 Ticket あたりの summary field、timestamp の選択、title truncation 長、default/max limit の具体値は実装者が current UI/LLM usage を見て決めてよい。
- detail-heavy mode は必要がある場合だけ明示 opt-in として追加してよいが、default tool result は軽量に保つ。
- CLI と tool output は同じ内部 summary model を共有してもよいし、human readability のために表示整形だけ分けてもよい。
- Docs/tool description/workflow guidance の更新範囲は、`TicketList` を selection/overview 用、`TicketShow` を詳細 authority と明示するために必要な範囲に限定してよい。

### Escalation conditions

- Orchestrator が `TicketList` だけで routing authority を持つ設計に変える必要が出た場合。
- Summary を削ることで panel / CLI / role workflow の既存の必須操作が成立しなくなる場合。
- Detail-heavy mode 追加が新しい public API / plugin / capability boundary の判断を要する場合。
- `limit` / truncation の方針が Ticket identity/base32 migration Ticket と衝突し、先後関係の判断が必要になった場合。

### Validation focus

- long title truncation。
- large list の default/max limit behavior。
- `state=all` / closed を含む listing cap。
- JSON/tool output shape が bounded summary であること。
- `TicketList` output に body/thread/artifacts の本文が漏れないこと。
- CLI list output が default で巨大化しないこと。
- `target/debug/yoi ticket doctor`、focused tests、`cargo fmt --check`、`git diff --check`。コード・resource 変更が入るため完了前に `nix build .#yoi` も確認する。

Open questions: なし。
Risk flags: `tool-output`, `context-budget`, `cli-ux`, `ticket-tools`, `docs-workflows`。
Related work: `20260609-073047-001` は Ticket/Objective ID 形式変更で非ブロッキング関連。ID 長や表示が変わっても、本 Ticket の summary/truncation 方針は canonical ID を不透明文字列として扱えば成立する。

---

<!-- event: intake_summary author: intake at: 2026-06-09T09:55:37Z -->

## Intake summary

TicketList の default output を LLM/context に安全な bounded summary へ軽量化する concrete work item として整理済み。詳細 authority は TicketShow に残し、List は selection/overview 用とする。Open questions はなく、risk flags は tool-output / context-budget / cli-ux / ticket-tools / docs-workflows。関連する ID 形式変更 Ticket は非ブロッキングで、canonical id は不透明文字列として扱う。

---

<!-- event: state_changed author: intake at: 2026-06-09T09:55:37Z from: planning to: ready reason: intake_ready field: state -->

## State changed

Intake refinement により、目的・受け入れ条件・binding decisions / invariants・implementation latitude・escalation conditions・validation focus が揃ったため、Orchestrator が routing できる ready に移行する。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-09T10:01:28Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-06-09T10:03:41Z -->

## Decision

Routing decision: waiting_for_tool_execution_context

This Ticket remains queued for now.

Reason:
- `20260609-093050-001` (`Tool実行にToolExecutionContextを渡す`) changes the core tool execution API.
- This Ticket changes `TicketList`, which is an LLM tool surface and should target the stable post-migration execution context API.
- Running both in parallel would create avoidable conflicts across tool schema/execution code.

State decision:
- Leave this Ticket `queued`.
- Do not create a worktree or spawn a coder yet.

Next action:
- Complete and merge `20260609-093050-001` first.
- Re-read this Ticket and accept from queued if still unblocked.

---

<!-- event: state_changed author: orchestrator at: 2026-06-09T10:19:34Z from: queued to: inprogress reason: orchestrator_acceptance_parallel_capacity field: state -->

## State changed

Accepted queued implementation under the updated parallel-capacity policy. The active ToolExecutionContext branch changes core Tool execution plumbing, while this Ticket changes the bounded TicketList output surface. There may be a small mechanical conflict around the TicketList tool implementation, but it is expected to be localized and manageable, so parallel work is preferred.

---

<!-- event: decision author: orchestrator at: 2026-06-09T10:19:34Z -->

## Decision

Routing decision: implementation_ready_parallel

Updated user instruction: prefer parallel work when Tickets are independent or expected conflicts are small/manageable.

Reason:
- This Ticket changes `TicketList` output/tool/CLI/workflow guidance.
- `20260609-093050-001` changes the core Tool execution context API. The overlap is expected to be a localized mechanical merge in tool execution signatures / TicketList implementation, not a design conflict.
- Running this in parallel should improve throughput while preserving separate worktrees and explicit review.

IntentPacket:

Intent:
- Make `TicketList` a lightweight bounded overview/selection surface and keep detailed authority in `TicketShow`.

Binding decisions / invariants:
- Default `TicketList` output must not include body/thread/artifact details.
- Per-Ticket summary must be short and bounded: canonical id, title, state, minimal timestamp, and short attention/blocking hints if any.
- Long titles/diagnostics must be truncated.
- Default/max limits should be smaller and context-safe, especially for `state=all` / closed-inclusive listing.
- Tool output is the priority for context safety; CLI should remain human-readable without huge default JSON/Markdown.
- `TicketList` is not routing/close/implementation authority; Orchestrator must read `TicketShow` for decisions.
- Do not change Ticket backend schema, TicketShow detail, Ticket relation/Objectives/OrchestrationPlan design.

Reviewer focus:
- Verify no body/thread/artifact prose leaks into list output.
- Verify output remains useful for selection and backlog overview.
- Verify default limits and truncation are bounded and tested.
- Watch for small merge interactions with the ToolExecutionContext branch; resolve by using the new context API without expanding scope.

Validation:
- Focused tests for long-title truncation, large-list limits, all/closed cap, JSON/tool output shape, no body/thread leakage.
- CLI list output tests.
- `cargo fmt --check`, `git diff --check`, `cargo run -q -p yoi -- ticket doctor`, `cargo check --workspace`, `nix build .#yoi`.

---
