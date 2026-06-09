<!-- event: create author: LocalTicketBackend at: 2026-06-08T13:22:30Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: plan author: intake at: 2026-06-09T00:16:33Z -->

## Plan

## Intake refinement

既存 Ticket は新規 umbrella Ticket 廃止の方針・背景・要件・受け入れ条件・非目標をすでに含んでおり、新規 Ticket 作成ではなくこの Ticket の refinement で足りる。

### Binding decisions / invariants

- 新規の長期 umbrella Ticket / progress container は作らない方針として文書化する。
- Ticket は単独で実装・レビュー・検証・完了判断できる concrete work item として扱う。
- 広い依頼の分割時は、進捗コンテナとしての umbrella を残すのではなく、concrete Tickets、Objective、typed Ticket relations、thread/decision 記録に責務を分ける。
- 既存 umbrella の退役 close は「関連作業がすべて完了した」ではなく「umbrella container の役割を退役した」ことを表す。
- `typed-ticket-relation-metadata` と `objective-records-for-medium-term-goals` の設計実装はこの Ticket の範囲外に残す。

### Implementation latitude

- 具体的な変更箇所は実装時の調査で選んでよいが、少なくとも開発/workflow docs、Intake/Planning guidance、Orchestrator guidance に umbrella Ticket を作らない・退役する方針が反映されている必要がある。
- 既存 umbrella の扱いは、代表例として `workspace-panel-orchestrator-queue-automation` に migration/close recommendation を残すか、明示的な follow-up に接続すればよい。

### Escalation conditions

- Objective または typed relation の current schema をこの作業内で設計・実装する必要が出た場合は scope creep として Orchestrator/人間判断へ戻す。
- 既存 Ticket の大量移行や historical thread rewrite が必要に見える場合は、この Ticket の非目標に反するため判断を戻す。
- umbrella 廃止方針が initial planning Ticket まで禁止する表現になりそうな場合は、受け入れ条件の「deprecated pattern is the long-lived umbrella progress container」を優先して調整する。

### Readiness

- readiness: implementation_ready
- risk_flags: [ticket, workflow, documentation, planning, orchestration]
- blocking open questions: none

Validation は既存 acceptance criteria の通り、`target/debug/yoi ticket doctor` と `git diff --check` を含める。実装が runtime resource / prompt / package source に触れる場合は通常の repository guidance に従って `nix build .#yoi` も検討する。

---

<!-- event: intake_summary author: intake at: 2026-06-09T00:16:38Z -->

## Intake summary

既存 Ticket を refinement し、新規 umbrella Ticket 作成ではなくこの Ticket を concrete documentation/workflow cleanup として進める方針を確認した。新規の長期 umbrella/progress-container Ticket を非推奨化し、広い依頼は concrete Tickets、Objective、typed Ticket relations、thread/decision 記録へ責務分離する。Objective/typed relation の設計実装、大量 historical migration、thread rewrite は範囲外。blocking open question はなく、implementation_ready として Orchestrator が routing 可能。

---

<!-- event: state_changed author: intake at: 2026-06-09T00:16:38Z from: planning to: ready reason: intake_ready field: workflow_state -->

## State changed

Intake refinement により、意図・受け入れ条件・binding decisions / invariants・implementation latitude・escalation conditions が揃ったため ready に遷移します。

---

<!-- event: decision author: hare at: 2026-06-09T01:00:46Z -->

## Decision

## Decision: no hierarchy relations

Umbrella Ticket deprecation also means not replacing umbrellas with Ticket hierarchy relations.

Do not introduce or use these relation concepts for current Ticket work:

- `parent` / `child`;
- `sub-ticket`;
- `umbrella`;
- `part_of` / `contains` decomposition semantics.

Concrete split work should become separate implementable Tickets. Medium-term context belongs in Objectives. Ticket relations should stay non-hierarchical: dependency, blocking, related, supersedes, duplicate/replacement, and similar direct relations.

---

<!-- event: state_changed author: workspace-panel at: 2026-06-09T01:14:14Z from: ready to: queued reason: queued field: workflow_state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-06-09T01:14:47Z -->

## Decision

Routing decision: defer_pending_policy_merge

This Ticket is queued and appears implementation-ready, but I am intentionally not accepting it to `inprogress` yet.

Reason:
- There is an approved merge-ready workflow/docs branch pending: `orchestrator-return-to-planning-context-policy` at commit `8576615`.
- `deprecate-umbrella-tickets` is also a workflow/docs/planning policy cleanup and is likely to touch adjacent files such as Orchestrator/Intake/Planning guidance and work-item docs.
- Running it before the pending policy branch is merged or otherwise resolved would create unnecessary conflict/review risk across the same maintained policy surfaces.

State decision:
- Leave this Ticket `queued`.
- Do not create a worktree or spawn a coder yet.

Next action:
- After `orchestrator-return-to-planning-context-policy` is merged/closed/cleaned up or explicitly held back, re-read this Ticket and accept it from queued if still unblocked.

---
