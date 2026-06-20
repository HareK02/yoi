## Resolution

`00001KVHX0WBE` を完了しました。

実装内容:
- Dashboard / Console / TUI terminology を導入しました。
  - Dashboard: `yoi panel` workspace cockpit/action surface。
  - Console: single-Pod chat/client surface。
  - TUI: terminal UI implementation umbrella。
- `yoi panel` command は維持しました。
- `yoi dashboard` alias は追加していません。
- `crates/tui/src/dashboard/` module boundary を追加しました。
  - `dashboard/mod.rs`
  - `dashboard/render.rs`
  - `dashboard/tests.rs`
- `crates/tui/src/console/` を single-Pod Console boundary として追加しました。
- `LaunchMode::Panel` は `dashboard::launch(...)` に routing され、Console/single-Pod entrypoint を経由しません。
- 旧 `single_pod.rs` / `multi_pod.rs` module route は Dashboard/Console boundary に置換しました。
- Help/docs/prompt wording を Dashboard / Console / TUI terminology に更新しました。
- Reviewer r1 で見つかった Dashboard behavior regression を修正しました。
  - recoverable nested Console failure は `yoi panel` を終了せず Dashboard loop を継続します。
  - successful Console return は live `DashboardApp` を fresh `load_app(...)` で置換せず、selection/draft/notices/diagnostics/local state を保持します。
  - regression tests を追加しました。

主な commit:
- `5415a947 tui: introduce dashboard console boundaries`
- `135343a2 tui: preserve dashboard after console return`
- `23ec2bbd merge: dashboard console tui refactor`

Review:
- r1 は nested Console open 後の Dashboard state/loop preservation regression で `request_changes`。
- Coder が `finish_nested_console_open(...)` と regression tests を追加。
- r2 は `approve`。

最終 validation:
- `cargo fmt --check`
- `git diff --check HEAD^1..HEAD`
- `cargo test -p tui`
- `cargo test -p yoi`
- `cargo check --workspace --all-targets`
- `cargo run -q -p yoi -- --help` targeted smoke。
- `test ! -e crates/tui/src/single_pod.rs`
- `test ! -e crates/tui/src/multi_pod.rs`
- targeted grep confirmed no `yoi dashboard` alias and no old Panel-to-Console/single_pod route。

Nix validation:
- Not run because this Ticket changed Rust/docs/prompt/module boundaries only and no package/source-filter/resource inclusion concern was found。

Validation log:
- `/run/user/1000/yoi/yoi-orchestrator/bash-output/bash-crXxMR.log`