TUI Dashboard の冗長な key hints と selected-row textual status display を削除し、Orchestrator worktree の `orchestration` branch に統合した。

主な成果:
- Top title line から `Row selection`, `blank Enter`, `Tab target` などの key hint guidance を削除。
- Selected Ticket / Pod / Intake / no-row selected textual status line を blank にした。
- Composer bottom actionbar を always-on key hints ではなく notices / diagnostics only に最小化。
- Row marker / highlighting rendering は維持。
- Keyboard/action behavior は変更していない。
- Unused selected-row display-status helper を削除。
- Dashboard render/unit tests を新 display contract に更新し、selected row marker の visibility assertion は維持。

統合・検証:
- Merge commit: `5abf16f9 merge: dashboard hint cleanup`
- Implementation commit: `03ad525f tui: trim dashboard redundant hints`
- Reviewer final verdict: approve
- Validation passed: `cargo fmt --check`, `git diff --check HEAD^1..HEAD`, `cargo test -p tui dashboard --lib`, `cargo test -p tui workspace_panel --lib`, and `cargo run -p yoi -- ticket doctor`。

範囲外:
- Console / single-Pod TUI hints は変更していない。
- Manual/PTY `yoi panel` visual check は実施していない。Focused render/unit tests を主 validation とした。