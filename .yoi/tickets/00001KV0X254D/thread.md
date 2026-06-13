<!-- event: create author: ticket-intake at: 2026-06-13T16:29:25Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: ticket-intake at: 2026-06-13T16:29:41Z -->

## Intake summary

ユーザー依頼に基づき、Panel Orchestrator の自動作成 orchestration branch 名を `.yoi/ticket.config.toml` の typed config として設定可能にする concrete Ticket を作成した。設定なしでは既存 default `orchestration/<workspace-orchestrator-pod-name>` を維持し、invalid / mismatched worktree は破壊的修復せず diagnostic で止める方針。blocking open question はない。

---

<!-- event: state_changed author: ticket-intake at: 2026-06-13T16:29:41Z from: planning to: ready reason: planning_ready field: state -->

## State changed

Intake 済み。要件・受け入れ条件・binding invariants・validation が揃っており、Orchestrator routing 可能。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-13T16:33:27Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-06-13T18:41:14Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Ticket は `queued` で、`.yoi/ticket.config.toml` に Panel Orchestrator の orchestration branch 名を typed config として追加する intent / requirements / acceptance criteria / invariants が具体化されている。
- `TicketRelationQuery` / `TicketOrchestrationPlanQuery` に blocker はない。
- Risk は config-schema / git-worktree / panel-orchestration だが、既存 default 維持、invalid branch の Git 操作前拒否、mismatched worktree の非破壊 diagnostic という安全境界が明記済み。
- 他 queued/inprogress との主な変更面は Panel orchestration worktree/config resolution で、E2E critical path・feature API cleanup・planning return action とは branch/worktree を分けて進められる。merge conflict が出た場合はその時点で止めて報告する。

Evidence checked:
- Ticket body / thread / artifacts。
- relation records: なし。
- orchestration plan records: なし。
- workspace state: Orchestrator worktree clean、dedicated child worktree で実装予定。

IntentPacket:
- `.yoi/ticket.config.toml` の typed config から Panel Orchestrator の resolved orchestration branch を取得し、worktree create / reuse / restore validation / diagnostics が同じ resolved value を使うようにする。

Binding decisions / invariants:
- 設定なしの default `orchestration/<workspace-orchestrator-pod-name>` は維持する。
- invalid branch は Git 操作前に拒否する。
- dirty/unknown/mismatched worktree を自動修復・削除・checkout しない。
- Ticket backend / Profile / prompt context への hidden injection ではなく明示 workspace config として扱う。

Validation:
- Focused ticket config / TUI orchestration tests、`cargo fmt --check`、`git diff --check`、`target/debug/yoi ticket doctor`。

---

<!-- event: state_changed author: orchestrator at: 2026-06-13T18:41:25Z from: queued to: inprogress reason: orchestrator_acceptance_unblocked field: state -->

## State changed

Ticket evidence、relation records、orchestration plan、workspace state を確認した。Blocking relation はなく、implementation-ready。実装 side effect の前に inprogress acceptance を記録する。

---

<!-- event: implementation_report author: hare at: 2026-06-13T18:59:21Z -->

## Implementation report

Implemented Ticket 00001KV0X254D.

Files changed:
- `crates/ticket/src/config.rs`
  - Added typed `[orchestration] branch = "..."` config support via `TicketOrchestrationConfig` and validated `GitBranchName`.
  - Kept default behavior as no configured branch; documented the optional config in the generated scaffold.
  - Added config validation tests for valid custom branches and invalid Git branch names.
- `crates/tui/src/multi_pod.rs`
  - Resolved Panel Orchestrator worktree branch from typed ticket config, falling back to `orchestration/<workspace-orchestrator-pod-name>`.
  - Reused the resolved branch for worktree creation, existing-worktree validation, restore preparation, queue handoff checks, and restore diagnostics.
  - Preserved refusal behavior for dirty/unknown/mismatched worktrees; mismatches are diagnostic and do not checkout/delete/repair.
  - Added TUI tests covering custom configured branch creation/reuse/restore, invalid branch rejection before worktree Git operations, and mismatched restore rejection without checkout.

Validation:
- `cargo test -p ticket config --lib` — passed.
- `cargo test -p tui ensure_and_restore_use_configured_orchestration_branch --lib` — passed.
- `cargo test -p tui invalid_configured_orchestration_branch_is_rejected_before_git_worktree_operations --lib` — passed.
- `cargo test -p tui restore_rejects_mismatched_configured_orchestration_branch_without_checkout --lib` — passed.
- `cargo fmt --check` — passed.
- `git diff --check` — passed.
- `cargo build -p yoi` — passed.
- `target/debug/yoi ticket doctor` — passed.

Additional note:
- I also tried broader `cargo test -p tui orchestration --lib`; it compiled but includes the existing `multi_pod::tests::orchestrator_launch_context_uses_orchestration_root_for_runtime_workspace` failure, in code not touched by this change. The focused new orchestration-branch tests listed above pass.

Commit:
- `1c54689e tui: configure orchestration branch`

Residual risks:
- Branch validation is implemented as an internal Git-ref compatible validator rather than invoking `git check-ref-format`, so it is intentionally conservative (for example, rejects spaces and full `refs/...` names) before any worktree Git operation.


---
