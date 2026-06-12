<!-- event: create author: "yoi ticket" at: 2026-06-12T01:16:39Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-06-12T02:39:25Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-12T02:39:25Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-12T02:39:25Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `yoi ticket` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-06-12T08:43:58Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Ticket body は Panel Queue action の目的、binding invariants、失敗時の block 条件、非目標、受け入れ条件を observable に記録している。
- `ready -> queued` は既に記録済みで、relation blocker はない。
- 設計境界は「Ticket tools は cwd 基準」「Panel は root/dev 側の Queue commit を authority とし、Orchestrator kick 前に orchestration worktree を ff-only sync する」「自動 conflict resolution / stash / rebase はしない」として十分に固定されている。
- 残る不確実性は既存 Panel/Ticket/git helper 実装への bounded implementation investigation と local tactic selection に閉じている。

Evidence checked:
- Ticket body / thread / artifacts: `00001KTWPE3KQ` の本文、state/event、artifact なしを確認。
- TicketRelationQuery: outgoing/incoming relation なし、blocker なし。
- TicketOrchestrationPlanQuery: 既存 record なし。今回 `orch-plan-20260612-084329-1` として accepted plan を記録。
- Workspace state: Orchestrator worktree は `## ticket/orchestrator-progress-companion-notify`、root workspace は `## develop`、どちらも dirty 出力なし。
- Code map: `crates/tui/src/workspace_panel.rs` と `crates/tui/src/multi_pod.rs` 周辺に Panel Queue / workspace panel / orchestrator launch の実装面があることを確認。
- Visible Pods: 現在 visible Pod はこの Orchestrator のみで、coder/reviewer は未起動。

IntentPacket:

Intent:
- Panel Queue action を、root/dev 側の `ready -> queued` commit と orchestration worktree への ff-only sync を完了してから Orchestrator notify/kick する durable handoff にする。

Binding decisions / invariants:
- root workspace の canonical top-level、orchestration worktree の canonical top-level、共通 git dir、期待 branch、dirty 状態、対象 Ticket の `ready` 状態、orchestration_head が root_head の ancestor であることを mutation 前に検証する。
- Queue commit は root/dev 側で対象 Ticket record だけを stage/commit する。
- orchestration worktree への自動同期は `git -C <orchestration_worktree> merge --ff-only <queue_commit_sha>` のみに限定する。
- merge commit、rebase、stash、patch apply、conflict resolution、dirty cleanup は Panel Queue action では行わない。
- Orchestrator notify/restore/kick は orchestration worktree HEAD が Queue commit を含み、orchestration worktree 側 Ticket backend で対象 Ticket が `queued` として読めることを確認した後だけ行う。
- Ticket tools が cwd 基準で動く設計と、workspace_root / cwd / orchestration worktree / merge target の分離を崩さない。

Requirements / acceptance criteria:
- Queue 成功時、Panel は queued Ticket id、dev 側 Queue commit sha、orchestration worktree sync 結果、Orchestrator notify/kick の有無を表示する。
- root dirty、orchestration dirty、branch divergence、non-ff sync required、Ticket state mismatch、worktree identity mismatch では Queue を block し、失敗した check 名と対象 path / branch / Ticket id を表示する。
- `nix build .#yoi` が通る。

Implementation latitude:
- 既存 Panel action / Git helper / Ticket backend 呼び出しのどこへ checks と sync 処理を分割するかは coder が調査して選んでよい。
- 表示文言や内部 helper 名は、失敗条件が具体的に伝わり、既存 UX と整合する範囲で調整してよい。
- Focused tests / unit coverage の追加位置は既存の TUI/client/test 構造に合わせてよい。

Escalate if:
- Queue action が root/dev 側 Ticket commit 以外の変更を commit/stage する必要が出る。
- ff-only 以外の sync、stash/rebase/conflict resolution、dirty cleanup を自動化しないと成立しない。
- Ticket tools の cwd 基準設計、Pod workspace_root/cwd 分離、dedicated Orchestrator worktree の authority boundary を変える必要が出る。
- Panel が Orchestrator routing/acceptance を代行する必要が出る。

Validation:
- 変更に対応する focused test / existing relevant cargo test。
- `cargo fmt --check`。
- `git diff --check`。
- `target/debug/yoi ticket doctor`。
- runtime resource / packaging / prompt へ触れる場合、または最終確認として `nix build .#yoi`。

Current code map:
- 主な調査対象: `crates/tui/src/workspace_panel.rs`, `crates/tui/src/multi_pod.rs`。
- Ticket backend / state mutation / CLI helper を変更する場合は `crates/ticket` と `crates/yoi` 側の既存 typed Ticket path に合わせる。
- Orchestrator/main `.yoi` project records、memory/local/runtime/secret-like `.yoi` paths は implementation worktree 側で勝手に生成・編集しない。

Critical risks / reviewer focus:
- root workspace と orchestration worktree を取り違えないこと。
- dirty/divergent 状態で mutation/commit/notify しないこと。
- Queue commit の差分が対象 Ticket record 以外へ広がらないこと。
- ff-only 限定を破らないこと。
- notify/kick が sync 完了前に走らないこと。
- cwd 基準 Ticket tools と workspace_root 分離を壊さないこと。

Next action:
- 今回の launch instruction は role Pod spawn を explicit follow-up まで待つ指定なので、ここでは `queued -> inprogress`、worktree 作成、coder/reviewer spawn、merge/close は行わない。
- 明示 follow-up があれば、side effect 前に TicketShow / relation / orchestration plan / git/worktree state を再確認し、問題がなければ `queued -> inprogress` を記録してから `multi-agent-workflow` に接続する。

---

<!-- event: state_changed author: orchestrator at: 2026-06-12T08:45:20Z from: queued to: inprogress reason: orchestrator_acceptance field: state -->

## State changed

Routing accepted for implementation. Ticket body/thread, relation blockers, orchestration plan, visible Pods, and root/orchestration git state were rechecked. No unresolved dependency/blocker or missing planning decision was found. Implementation side effects will start only after this accepted `queued -> inprogress` transition is recorded; worktree and sibling coder/reviewer routing will use the accepted plan `orch-plan-20260612-084329-1`.

---

<!-- event: plan author: orchestrator at: 2026-06-12T08:45:59Z -->

## Plan

Implementation worktree created for multi-agent handoff.

- Ticket: `00001KTWPE3KQ`
- Branch: `ticket/panel-queue-orchestrator-sync`
- Worktree: `/home/hare/Projects/yoi/.worktree/panel-queue-orchestrator-sync`
- Base: Orchestrator workspace HEAD `de0f533b` (`orchestration/yoi-orchestrator`), including the accepted routing record and `queued -> inprogress` transition.
- Next: spawn sibling coder with narrow write scope to the implementation worktree. Reviewer will be started after coder evidence is available.

---
