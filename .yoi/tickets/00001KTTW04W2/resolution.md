Orchestrator の明示 Ticket event を Companion に weak notify する形へ再実装した。

実装概要:
- 以前の Panel reload / periodic refresh 起点の Companion progress feed は削除済みで、今回の実装でも再導入していない。
- Orchestrator-role lifecycle Ticket tool の post-call event に限定して、live/reachable Companion peer へ `Notify { auto_run:false }` を送る。
- 対象 event は state change、comment/plan/decision/implementation_report、review、close/resolution 系の explicit mutating Ticket event。
- Passive Ticket reads/list/show/query では通知しない。
- missing/stopped/unreachable Companion は no-op とし、spawn/restore しない。
- Companion authority は増やしていない。
- Payload は Ticket id/title/state、event kind、short summary、`.yoi/tickets/<id>` ref 程度の bounded event notice に限定し、Ticket list snapshot、full thread、Pod output、diagnostics、provider error details、長大 log は含めない。
- LLM-facing notice framing は `resources/prompts/pod/ticket_event_companion_notice.md` に置き、Rust は bounded runtime values の構築と render に限定した。

Review / integration:
- Implementation commits:
  - `465ef100 feat: notify Companion on Orchestrator ticket events`
  - `6f8571f7 fix: render ticket event notice from prompt resource`
- Reviewer: `yoi-reviewer-event-companion-notify` が approve。
- Orchestrator merge commit: `2e5a60f4 merge: companion ticket event notify`
- Ticket completion commit: `ee6213ee ticket: mark event companion notify done`

Validation:
- `cargo test -p pod ticket_event_notify`: pass
- `cargo test -p pod ticket_event`: pass
- `cargo test -p pod weak_notify_to_live_peer_uses_notify_without_auto_run_and_noops_when_missing`: pass
- `cargo test -p tui companion_progress`: pass（0 matched; Panel feed remains absent）
- `rg` check confirmed no `companion_progress` / progress feed / `send_weak_notify` references in `crates/tui/src/multi_pod.rs`
- `cargo fmt --check`: pass
- `git diff --check HEAD~1..HEAD`: pass
- `./result/bin/yoi ticket doctor`: `doctor: ok`
- `nix build .#yoi`: pass

Cleanup:
- coder/reviewer Pods stopped。
- child worktree `/home/hare/Projects/yoi/.worktree/orchestrator-ticket-event-companion-notify` removed。
- branch `ticket/orchestrator-ticket-event-companion-notify` deleted。

Non-blocking note:
- Panel 非通知は TUI diff absence / `rg` check と focused behavior tests で確認した。将来の回帰防止として、Panel reload/open が Companion event notify を呼ばない明示 test を追加してもよい。
