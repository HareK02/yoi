<!-- event: create author: ticket-intake at: 2026-06-24T11:39:41Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-24T12:01:42Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-24T12:03:42Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Dashboard Queue により人間が Orchestrator routing を許可した queued Ticket として確認した。
- Ticket body は `yoi pod delete`, `yoi pod prune`, `yoi session prune --unreferenced` の command spelling、dry-run/force semantics、live Pod refusal、session history preservation、explicit age threshold、validation を具体的に列挙している。
- `TicketRelationQuery` は blocking relation 0 件、`TicketOrchestrationPlanQuery` は既存 plan 0 件だった。
- risk flags は pod-lifecycle / persistence / destructive-operation / cli-ux / session-history / authority-boundary だが、destructive operations の safety rails と escalation conditions が明記されている。risk は reviewer focus として扱えばよく、planning return 理由にはならない。
- 同時 queued Ticket `00001KVWPW3KX` は TUI Console rendering で code surface が別。conflict risk は低く、別 worktree/branch で並列開始可能。

Evidence checked:
- Ticket body / thread: `item.md`, `thread.md`。thread は create と ready->queued のみで未解決 blocker は記録されていない。
- Relations / orchestration plan: relation 0 件、routing 前 plan 0 件。accepted plan `orch-plan-20260624-120242-1` を記録済み。
- Code map: Grep で `crates/yoi/src/main.rs`, `crates/yoi/src/session_cli.rs`, `crates/pod-store/src/lib.rs`, `crates/session-store`, `crates/pod/src/entrypoint.rs`, `crates/pod/src/discovery.rs` 周辺を確認。
- Workspace state: `/home/hare/Projects/yoi/.worktree/orchestration` は clean。active inprogress Ticket は 0 件。

IntentPacket:

Intent:
- Pod/session storage を手動削除せずに安全に整理できる公式 CLI を追加し、Pod metadata delete / Pod prune / unreferenced session prune を dry-run-first、force-required、live-safe に実装する。

Binding decisions / invariants:
- No silent restore bypass。same-name fresh start は、ユーザーが明示的に stopped/restorable Pod metadata を削除した結果としてのみ発生する。
- `pod delete` は session logs/history を削除しない。
- live/reachable Pod metadata は削除しない。live 判定が不確実なら安全側に拒否する。
- old cleanup に暗黙 threshold を持たせない。`--older-than` など明示 criteria が必要。
- destructive deletion は `--force` 必須。`--dry-run` / default report を重視する。
- Pod metadata authority は `pod-store`、session log authority は `session-store` のまま。
- legacy top-level resume flags / bare Pod-name inference は再導入しない。
- Panel/TUI の broad Pod manager 化は non-goal。

Requirements / acceptance criteria:
- `yoi pod delete <NAME> [--force] [--dry-run]` で stopped/restorable Pod metadata を削除できる。
- live/reachable Pod delete/prune は拒否され理由を出す。
- `yoi pod prune --older-than <DURATION> [--force] [--dry-run]` は explicit threshold なしに old 判定削除しない。
- `yoi session prune --unreferenced [--older-than <DURATION>] [--force] [--dry-run]` は Pod metadata active pointer から参照されない session/segment を report/prune できる。
- delete/prune output は deleted/would delete/kept/refused reason を bounded に示す。
- focused tests が stopped Pod delete, live refusal, session preservation, unreferenced prune dry-run/force, threshold requirement, CLI parsing/help を cover する。

Implementation latitude:
- product CLI 側で management subcommands を捕捉するか、runtime entrypoint 側に安全に追加するかは coder が code map を見て判断してよい。
- 必要なら shared cleanup module や `session-store` delete API を追加してよい。path safety tests を伴うこと。
- Orphan detection は初期実装では active `PodMetadata.active.session_id` references を authority としてよい。lineage-aware retention は referenced sessions を削除しない限り follow-up に分けてよい。
- Output は human-readable でよい。JSON は自然なら追加してよいが必須ではない。

Escalate if:
- live Pod detection を安全に拒否できるほど reliable にできない。
- orphan detection が session lineage semantics の変更を必要とする。
- Pod delete の副作用として sessions を削除する必要が出る。
- storage migration / compatibility fallback が必要になる。
- command design が existing `yoi pod` runtime entrypoint usage と衝突する。
- cleanup が Panel role-session/Ticket claims, worktrees, branches, Ticket state を mutate しようとする。

Validation:
- `cargo fmt --check`
- focused `cargo test` for `yoi`, `pod-store`, `session-store`, affected Pod/discovery code
- `cargo check -p yoi -p pod -p pod-store -p session-store`
- `target/debug/yoi ticket doctor` または `yoi ticket doctor`
- `git diff --check`

Current code map:
- Primary: `crates/yoi/src/main.rs`, `crates/yoi/src/session_cli.rs`, `crates/pod-store/src/lib.rs`, `crates/session-store/src/fs_store.rs`, `crates/session-store/src/lib.rs`, `crates/pod/src/entrypoint.rs`, `crates/pod/src/discovery.rs`。
- Avoid: Panel/TUI manager UI, scheduler/stop semantics changes, Ticket/worktree/branch cleanup operations。

Critical risks / reviewer focus:
- accidental deletion of session history from `pod delete`。
- live/reachable Pod metadata deletion。
- unsafe path deletion or broad directory removal。
- force/dry-run semantics bypass。
- ambiguous age parsing/default threshold。
- breaking `yoi pod` runtime entrypoint spawn/restore behavior。

Next action:
- `queued -> inprogress` を記録してから worktree-workflow で dedicated worktree を作成し、Coder/Reviewer sibling loop に進める。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-24T12:04:07Z from: queued to: inprogress reason: orchestrator_acceptance_unblocked field: state -->

## State changed

Orchestrator acceptance: queued -> inprogress

- 直前確認で `TicketShow` は state `queued`、blocking relation は 0 件、accepted plan `orch-plan-20260624-120242-1` を確認した。
- 同時 queued Ticket `00001KVWPW3KX` は disjoint code surface のため別 worktree/branch で並列開始可能と判断した。
- routing decision と IntentPacket は Ticket thread に記録済み。
- これ以降に worktree-workflow で `/home/hare/Projects/yoi/.worktree/00001KVWPVHFJ-storage-cleanup-cli` を作成し、multi-agent-workflow に接続する。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-24T12:05:13Z -->

## Implementation report

Implementation orchestration update:

- worktree-workflow に従い、implementation worktree `/home/hare/Projects/yoi/.worktree/00001KVWPVHFJ-storage-cleanup-cli` と branch `work/00001KVWPVHFJ-storage-cleanup-cli` を作成した。
- child worktree では tracked `.yoi` project records を見える状態にし、`.yoi/memory`、local/runtime/log/lock/secret-like `.yoi` paths を sparse-checkout で除外する方針にした。
- Coder Pod `yoi-coder-00001KVWPVHFJ-cleanup-cli` を sibling として起動した。write scope は child worktree に限定し、runtime workspace root `/home/hare/Projects/yoi` は startup 要件を満たすため non-recursive read のみにした。
- Coder には root/original workspace と orchestration worktree で read/write/validation/git operation を行わないよう明示した。

---
