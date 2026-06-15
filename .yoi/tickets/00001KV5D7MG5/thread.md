<!-- event: create author: "yoi ticket" at: 2026-06-15T10:29:00Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-15T12:39:21Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-15T12:40:38Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Panel Queue により routing が明示的に許可され、Ticket は `queued`。
- Ticket body / thread / relation / OrchestrationPlan / Orchestrator workspace state / related closed Ticket context を確認した。blocking relation はなく、planning に戻す concrete missing information はない。
- `00001KV0X254D` で `.yoi/ticket.config.toml` の orchestration branch 設定が実装済みであり、`00001KV12W2RT` / `00001KV4ZPAD3` で Panel Ticket row / hierarchy 表示の前提が整っている。
- Risk は panel / ticket-state / orchestration / worktree / git-branch / read-only-overlay だが、Ticket 本文に safety checks・read-only invariant・fallback・action gating が明記されているため、残る不確実性は implementation tactic に閉じている。

Evidence checked:
- Ticket body/thread: overlay source、safety checks、display guidance、action gating、fallback、acceptance criteria、non-goals を確認。
- Ticket relations: blocker なし。
- OrchestrationPlan: 既存 record なし。
- Related context: `00001KV0X254D` closed（orchestration branch config）、`00001KV12W2RT` closed（two-line Ticket row/gate display）。
- Current config: `.yoi/ticket.config.toml` を確認。現 checkout には `[orchestration]` section がないため、default `<workspace>/.worktree/orchestration` / branch `orchestration` fallback を扱う必要がある。
- Orchestrator workspace: `/home/hare/Projects/yoi/.worktree/orchestration` は clean、`318aa191` 上。
- Visible Pods: implementation child Pod なし。

IntentPacket:

Intent:
- Workspace Panel の canonical current-branch Ticket state を上書きせず、configured orchestration worktree / branch の Ticket state を read-only overlay として読み、source-qualified progress と action gating を表示する。

Binding decisions / invariants:
- Primary Ticket state authority は current workspace branch の `.yoi/tickets` のまま。
- Overlay は read-only。overlay Ticket files を変更せず、current branch Ticket files に自動反映しない。
- current branch `state` を overlay state で上書きしない。
- Overlay source は expected path / branch / same repository / canonical top-level safety checks 通過時のみ使う。
- 誤った worktree / unrelated repository / branch mismatch の Ticket state を表示しない。
- Ticket identity join は canonical Ticket id。
- Overlay progress is source-qualified（例: `orchestration: inprogress`, `local: queued · orchestration: done`）。
- Overlay が current state より進んでいる場合、Queue/Start 等の duplicate action は safety 側で抑止する。
- Runtime overlay は merge/review/close authority の代替ではない。

Requirements / acceptance criteria:
- Panel ViewModel が configured orchestration worktree `.yoi/tickets` を read-only overlay として読み込める。
- `[orchestration]` config の `branch` / `worktree_dir` / `worktree_name` と defaults を使って expected path/branch を解決する。
- current branch state は overlay state で変更されない。
- local `queued` + overlay `inprogress` を Panel に overlay progress として表示する。
- local `queued` + overlay `done` を `orchestration: done` / merge pending 相当として表示し、Queue/Start を再提示しない。
- overlay unavailable / branch mismatch / missing worktree は panic せず current branch 表示へ fallback し、必要なら bounded diagnostic を出す。
- Tests cover overlay id join、queued+inprogress、queued+done action gating、branch mismatch ignore、missing worktree fallback、current branch Ticket files non-mutation。

Implementation latitude:
- Overlay-only Ticket の表示有無は実装判断。ただし表示する場合は source を明確にする。
- Exact wording / gate/action label は既存 Panel row hierarchy に合わせて調整してよい。
- Safety check helper の配置は existing Ticket config / Panel orchestration worktree code に合わせる。
- Heavy full doctor を毎 render 必須にしない。

Escalate if:
- Ticket config model に `worktree_dir` / `worktree_name` が存在せず、新 schema の exact authority が必要になる。
- same-repository / git-common-dir validation が current abstractions で安全に書けない。
- overlay state を読むために current workspace以外の arbitrary path authority が必要になる。
- action gating が lifecycle authority と衝突する。
- overlay-only Tickets の表示方針が product decision を要する。

Validation:
- `cargo test -p tui workspace_panel --lib` or focused overlay tests。
- Relevant ticket config / orchestration worktree tests if config parsing changes。
- `cargo check -p tui --all-targets`。
- `cargo fmt --check`。
- `git diff --check`。

Critical risks / reviewer focus:
- read-only overlay invariant。
- same-repository / branch / canonical path safety checks。
- source-qualified display; no state overwrite。
- duplicate Queue/Start action suppression when overlay has progressed。
- fallback diagnostics boundedness。
- no accidental merge/close authority substitution。
- row hierarchy and invalid Ticket tolerance regressions。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-15T12:40:45Z from: queued to: inprogress reason: orchestrator_acceptance field: state -->

## State changed

Routing decision と accepted implementation plan を記録済み。blocking relation / unresolved OrchestrationPlan blocker はなく、Orchestrator workspace は clean。implementation side effects の前に `queued -> inprogress` acceptance を記録する。

---

<!-- event: implementation_report author: yoi-coder-00001KV5D7MG5 at: 2026-06-15T12:57:12Z -->

## Implementation report

Implementation report from Coder Pod `yoi-coder-00001KV5D7MG5`:

Commit:
- `e0ddbed1 feat: show orchestration ticket overlay in panel`

Changed files:
- `crates/tui/src/workspace_panel.rs`
- `crates/tui/src/multi_pod.rs`

Implemented behavior:
- Panel ViewModel attempts to read a read-only orchestration worktree Ticket state overlay from configured/default `[orchestration]` settings.
- Overlay loading validates:
  - expected worktree path
  - expected branch
  - same Git common-dir/repository
  - canonical Git top-level equals expected worktree path
- Overlay state is joined by canonical Ticket id only.
- Local/current branch `.yoi/tickets` remains primary authority and is not mutated or overwritten.
- Rows with progressed overlay state show source-qualified state such as:
  - `local: queued · orchestration: inprogress`
  - `local: queued · orchestration: done`
- Progressed overlay state suppresses duplicate queue/start-like actions:
  - overlay `inprogress` is treated as active work / read-only progress
  - overlay `done` is treated as merge/review pending and does not expose queue/start again
- Orchestrator work-set derivation accounts for overlay in-progress/done guards to avoid duplicate start routing.

Tests added/covered:
- overlay joins by Ticket id
- local `queued` + overlay `inprogress` display/action behavior
- local `queued` + overlay `done` merge-pending/action gating
- branch mismatch ignored
- missing worktree fallback
- local Ticket file not mutated by overlay reading

Validation reported by coder:
- Passed: `cargo test -p tui workspace_panel --lib`
- Passed: `cargo check -p tui --all-targets`
- Passed: `cargo fmt --check`
- Passed: `git diff --check`

Repository status:
- Child implementation worktree clean after commit.

Residual blockers:
- None reported.

---

<!-- event: review author: yoi-reviewer-00001KV5D7MG5 at: 2026-06-15T13:04:06Z status: approve -->

## Review: approve

Review result: approve

Evidence checked:
- Inspected implementation diff `95abdc8d..e0ddbed1`.
- Changed files reviewed:
  - `crates/tui/src/workspace_panel.rs`
  - `crates/tui/src/multi_pod.rs`
- Worktree remained clean after read-only checks.

Acceptance review:
- Current branch remains primary authority:
  - local Ticket summaries are loaded from the current workspace backend first;
  - overlay data is joined by canonical Ticket id and stored separately in `PanelTicketInfo.orchestration_overlay`;
  - `workflow_state` remains local/current-branch state and is not overwritten by overlay.
- Overlay is read-only:
  - overlay loading uses `LocalTicketBackend::list_partial()` against the orchestration worktree ticket root;
  - no overlay writes or current-branch Ticket mutation path was found;
  - test coverage includes local Ticket files not being mutated by overlay read.
- Safety checks are appropriately conservative:
  - expected configured/default orchestration path;
  - path existence/directory check;
  - canonical top-level comparison;
  - expected branch check;
  - same git common-dir check against current workspace.
- Branch mismatch and missing worktree fall back safely.
- Default orchestration config behavior is covered when no `[orchestration]` section is present.
- Canonical Ticket id join is covered; unrelated overlay Tickets do not affect local rows.
- Source-qualified display is present, e.g. `local: queued · orchestration: inprogress/done`.
- Duplicate Queue/Start suppression is implemented for local `queued` with overlay `inprogress` or `done`.
- `multi_pod` queued-work attention also suppresses already-progressed overlay work.
- Overlay `done` / `closed` remains merge/review-pending information and does not grant close/merge/lifecycle authority.

Validation performed by reviewer:
- Passed: `git diff --check 95abdc8d..e0ddbed1`
- Passed: `cargo fmt --check`
- Passed: `git status --short` clean

Validation not run by reviewer:
- `cargo test -p tui workspace_panel --lib`
- `cargo check -p tui --all-targets`

Reason: review scope was read-only and Cargo writes build artifacts. Coder-reported passing results for those commands were inspected as evidence.

Risk assessment:
- Low residual risk. The main behavior is ViewModel-tested and safety checks are conservative.
- Minor UI risk: compact status column may truncate long source-qualified overlay text, but the full model value and disabled/detail text carry overlay progress context.

Conclusion:
- Approved. No blocker found.

---
