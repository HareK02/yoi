<!-- event: create author: ticket-intake at: 2026-06-14T14:56:51Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-14T15:35:56Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-14T15:36:50Z -->

## Decision

Routing decision: implementation_ready_but_waiting_capacity_conflict

Reason:
- Ticket body / thread / artifacts、relation、OrchestrationPlan、Orchestrator workspace state を確認した。要件・受け入れ条件・invariants は実装可能な粒度で揃っており、planning に戻す concrete missing information はない。
- ただし現在 `00001KV09WYC6` の Coder Pod が稼働中で、同じ Panel / `crates/tui/src/workspace_panel.rs` 周辺を主対象としている。
- 本 Ticket も Panel Ticket row / partial failure handling を扱うため、並行実装すると row model/action/diagnostic 周辺で conflict risk が高い。
- 既に `00001KTFY8V80` と `00001KV09WYC6` の2件が inprogress で Coder capacity を使用中。現時点では追加 spawn せず、`00001KV09WYC6` の結果を取り込んでから routing acceptance する。

Evidence checked:
- Ticket body/thread: ready -> queued を確認。
- Ticket relations: blocker なし。
- OrchestrationPlan: 既存 record なし。
- Orchestrator workspace: `/home/hare/Projects/yoi/.worktree/orchestration` は clean、queue commit `28f3ed62` 上。
- Visible Pods: `yoi-coder-00001KTFY8V80` と `yoi-coder-00001KV09WYC6` が running。

Next action:
- `00001KV09WYC6` の実装・review・integration 後、Panel row model/action surface を再確認してから `queued -> inprogress` acceptance を検討する。
- planning return ではなく queued のまま waiting とする。

---

<!-- event: decision author: yoi-orchestrator at: 2026-06-14T15:57:39Z -->

## Decision

Routing decision: implementation_ready

Reason:
- 以前の waiting reason は `00001KV09WYC6` と同じ Panel row/action surface の conflict risk だったが、`00001KV09WYC6` は reviewer approve、orchestration branch への merge、focused validation、Ticket `done` まで完了した。
- Ticket body / thread / relations / orchestration plan / current Orchestrator workspace を再確認した。blocking relation はなく、planning に戻す concrete missing information はない。
- 本 Ticket は invalid/corrupt/unreadable individual Ticket record に対する Panel partial failure handling として concrete であり、残る不確実性は backend/list/show error handling と Panel row/diagnostic 表現の実装戦術に閉じている。

Evidence checked:
- Ticket body/thread: Background, requirements, acceptance criteria, invariants, implementation latitude, escalation conditions, validation を確認。
- Ticket relations: blocker なし。
- OrchestrationPlan: `00001KV09WYC6` との prior conflict/waiting note を確認。先行 Ticket 完了により blocker は解消。
- Orchestrator workspace: `/home/hare/Projects/yoi/.worktree/orchestration` は clean、`81667a9a` 上。
- Active Pods: `00001KTFY8V80` reviewer running、coder idle。Panel implementation worker/reviewer for `00001KV09WYC6` は停止済み。
- Current code map after prior Panel merge: `crates/tui/src/workspace_panel.rs`, `crates/tui/src/multi_pod.rs`, `crates/ticket/src/lib.rs`。

IntentPacket:

Intent:
- Workspace Panel で個別 invalid/corrupt/unreadable Ticket record があっても、正常な Ticket rows と actions を表示・維持し、invalid record は bounded diagnostic/placeholder として見せる。

Binding decisions / invariants:
- invalid Ticket を理由に正常 Ticket の Panel 操作を巻き添えで止めない。
- invalid Ticket record を Panel が自動修復・自動削除しない。
- invalid Ticket には Queue / Close / planning return など lifecycle mutation action を出さない。
- Ticket lifecycle authority / state schema は変更しない。
- Ticket backend config 全体が unusable な場合と、個別 record の partial failure を区別する。
- 正常 Ticket の lifecycle mutation は既存 typed Ticket backend / Panel action path を通す。
- invalid record の content や secret-like content を UI/diagnostic に漏らさない。

Requirements / acceptance criteria:
- valid + invalid Ticket が混在しても valid rows は残る。
- 正常 ready Ticket の Queue action、正常 planning Ticket の clarification/Intake 導線を維持する。
- invalid Ticket は bounded diagnostic または disabled placeholder row として見える。
- invalid Ticket に lifecycle mutation action を提示しない。
- Panel header/diagnostics は全体 unavailable ではなく一部読み込み失敗を表す。
- backend root/config unusable の既存 degraded behavior は壊さない。
- Focused tests で partial failure、bounded invalid indication、valid action preservation、config unusable case を確認する。

Implementation latitude:
- 表示形式は header diagnostic / placeholder row / detail route のどれでもよい。
- backend `list` を lossy にするか、Panel 側 per-Ticket load recovery にするかは実装判断。ただし typed boundary を保ち、Panel 専用 ad hoc parsing で schema authority を迂回しない。
- `TicketDoctor` logic を再利用してよいが、Panel 起動ごとに重い full doctor を必須にしない。

Escalate if:
- `TicketBackend::list` public semantics の大幅変更が必要。
- invalid path/id を安全に特定できない。
- Panel action dispatch が valid Ticket と invalid placeholder を安全に分けられない。
- TicketDoctor と Panel diagnostics の severity/wording が矛盾する。
- invalid content を読まないと UI 表示できない設計になる。

Validation:
- `cargo test -p tui workspace_panel --lib`
- 必要に応じて `cargo test -p ticket`
- `cargo fmt --check`
- `git diff --check`

Critical risks / reviewer focus:
- partial failure が全体 Ticket UI unavailable に戻らないこと。
- invalid placeholder/action key が lifecycle mutation path に入らないこと。
- Ticket backend config failure との区別。
- diagnostics の boundedness と secret-like content 非露出。
- prior `00001KV09WYC6` の Ticket-associated Intake row behavior との整合。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-14T15:57:47Z from: queued to: inprogress reason: orchestrator_acceptance_after_conflict_resolution field: state -->

## State changed

Routing decision と accepted implementation plan を記録済み。先行 `00001KV09WYC6` は merge/validation/done 済みで、prior conflict/waiting reason は解消。blocking relation / unresolved orchestration-plan blocker はないため、implementation side effects の前に `queued -> inprogress` acceptance を記録する。

---

<!-- event: implementation_report author: yoi-coder-00001KV3A5CNH at: 2026-06-14T16:21:50Z -->

## Implementation report

Implementation report from Coder Pod `yoi-coder-00001KV3A5CNH`:

Commit:
- `b83b9e4e fix: tolerate invalid ticket rows in panel`

Changed files:
- `crates/ticket/src/lib.rs`
  - Added tolerant `LocalTicketBackend::list_partial(...)` and `show_partial(...)`.
  - Added bounded/sanitized `TicketInvalidRecord` reporting.
  - Preserved strict existing `TicketBackend::list/show` semantics.
  - Added ticket backend test for valid records surviving peer invalid record failures.
- `crates/tui/src/workspace_panel.rs`
  - Panel now uses partial ticket loading.
  - Valid Ticket rows remain visible/actionable when sibling Ticket records are invalid.
  - Invalid records render as disabled diagnostic placeholder rows.
  - Invalid placeholder rows have no lifecycle actions and no `ticket` action identity.
  - Header diagnostics indicate partial Ticket load failure with bounded placeholder count.
  - Added focused Panel tests covering valid ready Queue action, valid planning Clarify/Intake path, associated Intake row adjacency, invalid row bounded/non-actionable behavior, secret-like content non-exposure, and backend config unusable behavior.
- `crates/tui/src/multi_pod.rs`
  - Added rendering/selection support for invalid Ticket placeholder rows.
  - Invalid placeholder rows are shown as ticket-section diagnostics but remain action-disabled.

Validation reported by coder:
- Passed: `cargo test -p tui workspace_panel --lib`
- Passed: `cargo test -p ticket partial_list_and_show --lib`
- Passed: `cargo fmt --check`
- Passed: `git diff --check`

Repository status:
- Child implementation worktree clean after commit.

Residual risks / blockers:
- None known.
- Strict backend list/show behavior was preserved; new partial methods are opt-in for Panel tolerance.

---
