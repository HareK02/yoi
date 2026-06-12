Orchestrator Idle 時に queued Ticket を見落とさない bounded attention layer を実装した。

実装概要:
- `crates/tui/src/multi_pod.rs` に session-scoped `OrchestratorWorkSet` を追加し、Panel reload 境界で queued/actionable/planned/active_inprogress を導出するようにした。
- Idle かつ reachable な Orchestrator Pod にだけ bounded queued-work attention を送る。
- `active_inprogress` がある場合は re-kick を抑制し、waiting reason を session work set / Panel header detail に保持する。
- Duplicate-start guard は Ticket id に紐づく local claim / related visible Pods / worktree 表示情報から導出する。
- Session work set は `MultiPodApp` 内 runtime/session state に留め、durable per-Ticket artifact store は追加していない。
- `resources/prompts/panel/orchestrator_idle_queue_notice.md` を追加し、Orchestrator attention payload の prompt 本文を resource 化した。
- Scheduler loop、polling loop、queue drain loop、core Ticket state、implementation side effect、acceptance gate bypass は追加していない。

Review / integration:
- Implementation commit: `d2fae81a tui: add idle queued orchestrator attention`
- Reviewer: `yoi-reviewer-idle-queued-rekick` が approve。
- Orchestrator merge commit: `9538feb1 merge: idle queued orchestrator attention`
- Ticket completion commit: `60cf2d9f ticket: mark idle queued done`

Validation:
- `cargo test -p tui queued_attention`: pass, 3 tests
- `cargo test -p tui planned_queued_prompts`: pass, 1 test
- `cargo test -p tui rediscovered_queued_work`: pass, 1 test
- `cargo test -p tui active_inprogress_suppresses`: pass, 1 test
- `cargo test -p tui idle_orchestrator_gets_bounded_attention`: pass, 1 test
- `cargo test -p tui workspace_panel`: pass, 12 tests
- `cargo check -p tui`: pass
- `cargo fmt --check`: pass
- `git diff --check HEAD~1..HEAD`: pass
- `./result/bin/yoi ticket doctor`: `doctor: ok`
- `nix build .#yoi`: pass

Known broad-suite failures:
- Existing broad `cargo test -p tui` failures noted in thread remain outside this Ticket and were not blockers for focused implementation/review.

Cleanup:
- coder/reviewer Pods stopped。
- child worktree `/home/hare/Projects/yoi/.worktree/orchestrator-idle-queued-rekick` removed。
- branch `ticket/orchestrator-idle-queued-rekick` deleted。

Non-blocking risks:
- Failed attention delivery is not fingerprint-marked, so a repeatedly reachable-but-rejecting socket can retry on subsequent Panel reloads; this is consistent with existing notice behavior but may be noisy。
- Duplicate guard is bounded to Panel-visible claim/Pod/worktree derivation and is not a full global scheduler/lock, matching this Ticket の scoped attention model。