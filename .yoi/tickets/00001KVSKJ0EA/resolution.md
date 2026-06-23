Dashboard の row selection を明示的 user action の結果として扱うように修正した。

完了内容:
- 初期表示で visible row があっても自動選択しない。
- reload / snapshot reconciliation で `selected_row = None` を勝手に selection へ戻さない。
- `Esc` による no-selection が reload を跨いで維持される。
- 選択中 row が消えた場合は `None` へ安全に落とす。
- keyboard navigation では no-selection から明示的に selection を作成できる。
- `list.selected_name` は実際の Pod row selection と同期し、no-selection / non-Pod selection では clear する。
- no-selection + `TicketIntake` composer submit が既存 Ticket refinement ではなく global/new Intake route へ進むことを focused test で確認した。

統合:
- Implementation: `5c242d96 fix: keep dashboard row selection explicit`
- Merge: `58904c44 merge: dashboard no auto selection`

検証:
- Reviewer approval: `yoi-reviewer-00001KVSKJ0EA-r1`
- `cargo fmt --check`: passed
- `git diff --check HEAD^1..HEAD`: passed
- `cargo test -q -p tui workspace_panel`: passed (`27 passed`)
- `cargo test -q -p tui dashboard`: passed (`111 passed`)
- reviewer 側追加確認 `cargo test -q -p tui`: passed (`372 passed`)
- `cargo run -q -p yoi -- ticket doctor`: passed (`doctor: ok`)

残作業:
- なし。