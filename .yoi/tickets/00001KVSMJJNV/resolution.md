Ticket 00001KVSMJJNV は完了。

実装内容:
- TUI Console の `Paused` 状態で `Ctrl+X` を TUI/Pod shutdown ではなく `Method::Cancel` として扱うようにした。
- `Idle` の `Ctrl+X` shutdown behavior と、`Running` の cancel behavior は維持した。
- paused cancel 後に resume 可能な interrupted state が残らず、次の入力が fresh run になるよう controller / Pod lifecycle を更新した。
- paused cancel を normal `RunCompleted { Finished }` として永続化せず、typed session log entry `LogEntry::PausedTurnAbandoned { ts }` として記録するようにした。
- replay / restore で `PausedTurnAbandoned` が `last_run_interrupted` を clear するようにした。
- focused tests を追加・更新し、TUI key handling、controller behavior、session log JSON/replay semantics、fake normal-finished run が残らないことを確認した。

主要 commits:
- `90b1a1fc tui: cancel paused turns with ctrl-x`
- `8c8fb014 fix: log paused cancel lifecycle explicitly`
- `76c80054 merge: 00001KVSMJJNV paused ctrl-x cancel`

Review:
- 初回 review は lifecycle log semantics の不一致で request_changes。
- `PausedTurnAbandoned` による typed lifecycle representation を追加後、Reviewer が approve。

Validation:
- `cargo test -p session-store paused_turn_abandoned -- --nocapture`
- `cargo test -p pod paused_cancel_abandons_resume_and_next_input_is_fresh_run -- --nocapture`
- `cargo test -p tui ctrl_x -- --nocapture`
- `cargo fmt --check`
- `git diff --check HEAD~1..HEAD`

Merge note:
- orchestration branch で実装・Ticket 記録・close resolution をまとめ、merge target へ fast-forward 予定。