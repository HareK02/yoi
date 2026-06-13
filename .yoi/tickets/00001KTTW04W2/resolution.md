Orchestrator progress を AutoKick なしで live/reachable Companion に通知する仕組みを実装した。

実装概要:
- `Method::Notify { auto_run: bool }` を追加し、`auto_run: false` では idle Pod に `RunForNotification` を stage しない weak notification にした。
- `auto_run: true` と legacy missing-field behavior は既存 Notify と互換にした。
- Panel から live/reachable Companion へ bounded progress notice を `Notify { auto_run: false }` で送るようにした。
- missing/stopped/unreachable Companion は best-effort no-op とし、spawn/restore しない。
- Progress summary は Ticket id/title/state、role pod status、short reason、`.yoi/tickets/<id>` refs に限定し、full thread、Pod output、diagnostics、provider errors、secret-like content を含めない。
- Panel に Companion progress freshness / last-updated indication を追加した。
- Reviewer request_changes を受け、Companion progress notice の LLM-facing framing を Rust 直書きから `resources/prompts/panel/companion_progress_notice.md` へ移し、Rust は bounded runtime values の rendering に限定した。
- Companion profile/tool authority は変更していない。

Review / integration:
- Implementation commits:
  - `a87d3154 feat: weak companion progress notify`
  - `61e6c068 fix: resource-back companion progress notice`
- Reviewer: `yoi-reviewer-companion-progress-notify` が初回 request_changes、fix 後 approve。
- Orchestrator merge commit: `56b10a2d merge: companion weak progress notify`
- Ticket completion commit: `2b64f428 ticket: mark companion notify done`

Validation:
- `cargo test -p protocol`: pass, 39 tests
- `cargo test -p pod --test controller_test`: pass, 36 tests
- `cargo test -p tui companion_progress -- --nocapture`: pass, 6 tests
- `cargo test -p tui send_notify_only_can_deliver_weak_notification_without_auto_run -- --nocapture`: pass, 1 test
- `cargo check -p protocol -p pod -p tui`: pass
- `cargo fmt --check`: pass
- `git diff --check HEAD~1..HEAD`: pass
- `./result/bin/yoi ticket doctor`: `doctor: ok`
- `nix build .#yoi`: pass

Known unrelated broad-suite failures:
- Existing prompt/TUI broad-suite failures noted in thread remain outside this Ticket and were not blockers for focused implementation/review.

Cleanup:
- coder/reviewer Pods stopped。
- child worktree `/home/hare/Projects/yoi/.worktree/orchestrator-progress-companion-notify` removed。
- branch `ticket/orchestrator-progress-companion-notify` deleted。

Non-blocking risk:
- Added `minijinja` dependency to `crates/tui`; it is already used elsewhere in the workspace, and `Cargo.lock` / `package.nix` were updated with passing Nix build.