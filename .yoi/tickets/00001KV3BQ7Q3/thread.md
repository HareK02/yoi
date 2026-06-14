<!-- event: create author: ticket-intake at: 2026-06-14T15:24:05Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-14T15:35:57Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-14T15:36:50Z -->

## Decision

Routing decision: implementation_ready_but_waiting_capacity_conflict

Reason:
- Ticket body / thread / artifacts、relation、OrchestrationPlan、Orchestrator workspace state を確認した。validation / E2E evidence work item として具体化されており、planning に戻す concrete missing information はない。
- ただし現在 `00001KV09WYC6` が Panel 表示 surface を変更中で、`00001KV3A5CNH` も Panel Ticket row / diagnostics を扱う queued item として待機中。
- 本 Ticket は現行 HEAD / 現行 E2E infrastructure で Panel/TUI user-visible behavior を確認する性質のため、同じ Panel/TUI surface の実装 branch と並行すると、どの HEAD を evidence 対象にしたかが曖昧になりやすい。
- 既に2件の Coder Pod が running であり、現時点では追加 spawn せず、先行 Panel/TUI changes の integration 状態を待つ。

Evidence checked:
- Ticket body/thread: ready -> queued を確認。
- Ticket relations: blocker なし。
- OrchestrationPlan: 既存 record なし。
- Orchestrator workspace: `/home/hare/Projects/yoi/.worktree/orchestration` は clean、queue commit `28f3ed62` 上。
- Visible Pods: `yoi-coder-00001KTFY8V80` と `yoi-coder-00001KV09WYC6` が running。

Next action:
- 先行 inprogress Ticket のうち Panel/TUI surface に触れる `00001KV09WYC6` の outcome を確認し、必要なら `00001KV3A5CNH` も先に処理したうえで、本 Ticket の E2E evidence 対象 HEAD を明確化してから `queued -> inprogress` acceptance する。
- planning return ではなく queued のまま waiting とする。

---

<!-- event: decision author: yoi-orchestrator at: 2026-06-14T16:39:14Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Prior waiting reason は Panel/TUI surface の先行 implementation branch により E2E evidence 対象 HEAD が曖昧になることだった。
- `00001KV09WYC6` と `00001KV3A5CNH` は reviewer approve、orchestration branch merge、focused validation、Ticket `done` まで完了した。
- Ticket body / thread / relations / orchestration plan / current Orchestrator workspace を再確認した。blocking relation はなく、planning に戻す concrete missing information はない。
- 本 Ticket は validation/evidence work item であり、現行 orchestration HEAD で対象 TUI/Panel behavior を E2E で pass/fail/coverage-gap として記録する作業に閉じている。

Evidence checked:
- Ticket body/thread: 対象 commit 3件、確認すべき user-visible behavior、acceptance criteria、binding decisions、escalation conditions、validation command を確認。
- Ticket relations: blocker なし。
- OrchestrationPlan: prior waiting note と `after 00001KV09WYC6` を確認。先行 Panel work は完了済み。
- Orchestrator workspace: `/home/hare/Projects/yoi/.worktree/orchestration` は clean、`765e6e8e` 上。
- Visible Pods: child Pod なし。

IntentPacket:

Intent:
- 現行 orchestration HEAD / 現行 E2E infrastructure で、対象 TUI/Panel merge commit が意図した user-visible behavior を実プロセス PTY 経路で確認し、pass/fail/coverage-gap を明示する。

Binding decisions / invariants:
- focused unit test / code review だけで user-visible TUI/Panel behavior を確認済み扱いにしない。
- E2E pass と manual/live user confirmation を混同しない。
- 現行 E2E が確認していない behavior を pass と書かない。
- historical merge decision を書き換えず、現行状態の evidence を追加する。
- 主目的は validation/evidence 整理であり、対象 behavior の大きな再設計や unrelated fix はしない。
- E2E 追加・更新時は fixture-local HOME/XDG/runtime/workspace isolation と no-provider/no-network 前提を維持する。

Requirements / acceptance criteria:
- `802fa1f0`, `02311883`, `db7bad7a` の3件それぞれについて、現行 E2E での確認結果を pass / fail / coverage gap として記録する。
- pass の場合は E2E test 名、assertion、command、結果を記録する。
- coverage gap の場合は何が確認できないか、追加 E2E か manual/live validation が必要かを記録する。
- mouse selection は実 `yoi` binary + PTY 経路で user-visible observer を確認する。
- quit latency は process exit だけでなく pending work / threshold / latency 観点で何を保証したか明示する。
- rewind live refresh は restart/restore なしの live 表示更新が E2E で確認されるか、不足を明示する。
- `cargo test -p yoi-e2e --features e2e` または同等の現行 E2E command を実行し結果を記録する。

Implementation latitude:
- 既存 `yoi-e2e` scenario の再利用、test name/assertion の明確化、最小限の scenario 追加・更新は Coder 判断。
- 不足する observer/helper は production behavior に影響しない `e2e-test` feature gate 配下で追加可。
- latency threshold は既存 E2E 基準を優先し、変更が必要なら理由を報告する。
- flake の hardening と behavior fix を混同しない。

Escalate if:
- 現行 E2E infrastructure では原理的に確認不能で、実端末 manual validation や新 harness 設計が必要。
- latency の測定が既存 observer / threshold では意味を持たない。
- PTY SGR injection と実端末 mouse 操作に乖離疑いが残る。
- actual regression が見つかり、validation Ticket の範囲を超える修正が必要。

Validation:
- `cargo test -p yoi-e2e --features e2e` または必要な narrow E2E command。
- E2E 追加・更新時は `cargo test -p yoi-e2e --no-run`, `cargo fmt --check`, `git diff --check`。
- 変更範囲に応じて `cargo check -p yoi-e2e -p yoi -p tui`。

Critical risks / reviewer focus:
- PTY / real binary 経路であること。
- unit/focused test と E2E evidence の混同防止。
- coverage gap を pass と偽らないこと。
- E2E fixture isolation / no-provider / no-network 維持。
- recently merged Panel changes (`00001KV09WYC6`, `00001KV3A5CNH`) 後の現行 HEAD に対する evidence であること。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-14T16:39:19Z from: queued to: inprogress reason: orchestrator_acceptance_after_panel_head_stabilized field: state -->

## State changed

Routing decision と accepted implementation/evidence plan を記録済み。先行 Panel/TUI implementation Tickets は merge/validation/done 済みで、prior waiting reason は解消。blocking relation / unresolved orchestration-plan blocker はないため、E2E validation side effects の前に `queued -> inprogress` acceptance を記録する。

---
