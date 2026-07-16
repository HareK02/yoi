# テスト妥当性レビュー: tui

- 評価: 混在

## 確認範囲

- 対象 crate: `crates/tui`
- 確認した主な範囲:
  - `crates/tui/Cargo.toml`
  - `crates/tui/README.md`
  - `crates/tui/src/lib.rs`
  - 既存 unit tests を持つ主要 module:
    - `app.rs`
    - `input.rs`
    - `multi_pod.rs`
    - `single_pod.rs`
    - `workspace_panel.rs`
    - `pod_list.rs`
    - `task.rs`
    - `command.rs`
    - `markdown.rs`
    - `ui.rs`
    - `spawn.rs`
    - `setup_model.rs`
    - `composer_history.rs`
    - `composer_keys.rs`
    - `role_session_registry.rs`
    - `picker.rs`
    - `keys.rs`
- crate の責務として確認した内容:
  - `ratatui` / `crossterm` ベースの TUI 表示・入力処理
  - single-pod chat UI、multi-pod / workspace panel、pod picker、spawn/setup/key management UI
  - composer/input model、Markdown/tool/task/system event rendering
  - Pod list / Ticket panel / local role-session claim などの表示用 view model glue

## 現在のテストがよくカバーしていること

- Unit test の量は多く、`cargo test -p tui` 実行時に 299 tests が列挙される。TUI crate としては model-level の回帰テストがかなり厚い。
- `app.rs` / `input.rs` は重要な composer invariants をよく押さえている。
  - running 中の submit queueing
  - rollback 時の入力復元
  - input history persistence
  - file completion
  - typed `Segment` の保持
  - context usage 表示
  - live system item / task snapshot の反映
- `multi_pod.rs` / `workspace_panel.rs` は dogfooding 上重要な panel semantics を広く検証している。
  - Ticket row ordering / state display
  - ready ticket queue action
  - done ticket close action
  - Review action が黙って approve しないこと
  - Companion / Orchestrator lifecycle decision
  - composer が selected pod ではなく workspace companion に向くこと
  - local role-session claim の扱い
- `pod_list.rs` は live/stored pod の分類・sort・socket reachability・corrupt metadata diagnostic を押さえており、TUI が誤って restore/send 可能扱いしないための invariant として有用。
- `task.rs` には `snapshot_format_contract` があり、Pod 側 snapshot 形式と TUI 側 deserialization の境界を守るテストとして妥当。
- `command.rs` / `single_pod.rs` は `:compact` / `:rewind` / `:peer` などの command が通常 user message として送られないこと、local diagnostic になることを確認していて良い。
- `markdown.rs` は Markdown renderer の基本構文、list、code block、table を文字列化して確認しており、LLM 出力表示の最低限の regressions には効く。
- `composer_history.rs` / `role_session_registry.rs` は workspace-scoped path、duplicate suppression、claim conflict など、永続化先を誤らないための安全側テストがある。

## 不足 / 疑問のあるテスト

- 現在の test suite は red。`cargo test -p tui` は 296 passed / 3 failed。
  - `multi_pod::tests::orchestrator_launch_context_uses_orchestration_root_for_runtime_workspace`
    - 失敗内容: expected `"/repo/yoi/.worktree/orchestration/yoi-orchestrator"` but actual `"/repo/yoi"`
    - 現在の設計では dedicated Orchestrator が process cwd と runtime workspace を分ける可能性があり、この test 名・期待値は仕様の変化に追随していない疑いが強い。
  - `spawn::tests::profile_choices_use_project_registry_default`
  - `spawn::tests::profile_choices_include_builtin_and_project_default_marker`
    - 失敗内容: selected index expected `1`, actual `6`
    - builtin profiles の数・順序に依存した brittle な assertion になっている。profile selector / default marker / selected profile identity を見るべきで、index 固定は妥当性が低い。
- UI renderer の実画面に対する coverage は限定的。
  - `ui.rs`, `multi_pod.rs`, `tool.rs` には多数の render helper があるが、terminal buffer snapshot 的な検証は少ない。
  - 文字列 helper の unit test はある一方、layout collapse、small terminal width/height、wide Unicode、style priority、scroll offset と描画結果の統合的な確認は薄い。
- `tool.rs` の test が見当たらないのは大きな穴。
  - Read/Write/Edit/Search/Bash などの tool call rendering は日常的に目に入る UI であり、diff wrapping、capped output、error suffix、aggregate Read rendering などの regression が入りやすい。
- 実 terminal / event loop / crossterm raw mode まわりはほぼ unit-test 対象外。
  - project instruction 上 E2E は未設計なのでやむを得ないが、`run_loop`, polling, resize, alternate screen cleanup, Ctrl-C/quit guard の実端末統合は unit tests だけでは保証できない。
- async / process interaction は一部 mock・TempDir・git repo でよく検証されているが、実 Pod process / socket / session log replay との E2E はない。
  - 特に Panel から child Pod launch、attach、completion notification、reload の end-to-end 整合は crate 単体の test だけでは判断できない。
- 一部の panel/render tests は text alignment や label の exact string に寄っており、仕様化された文言なら有用だが、見た目調整で壊れやすい。重要 invariant と cosmetic expectation を分けると保守性が上がる。

## 追加を提案するテスト

- まず red tests を仕様に合わせて直す。
  - Orchestrator launch context は `workspace_root`, `role_workspace_root`, `original_workspace_root`, `implementation_worktree_root`, `merge_target_workspace_root`, process cwd のどれを守る test なのかを明確にして、名前と assertion を更新する。
  - Profile choice tests は index ではなく、selected choice の `selector` / `source` / `is_default` 相当の意味で assert する。
- `tool.rs` に focused unit tests を追加する。
  - Edit diff の old/new wrapping
  - multiline result の cap
  - failed tool call の error line
  - Bash output truncation metadata
  - Read aggregate rendering
  - narrow width と wide Unicode
- `ratatui::backend::TestBackend` を使った small snapshot tests を少数追加する。
  - single-pod main screen の最小 smoke
  - multi-pod panel の ticket row + companion composer
  - task side pane / mini view
  - very small terminal size で panic しないこと
- scroll / viewport invariant の test を増やす。
  - history scroll と rewind picker scroll が干渉しないことは既にあるが、tool blocks、Markdown long output、task pane open/close、terminal resize 時の offset clamp も対象にする。
- 実 Pod process まで含む E2E が未設計なら、まずは crate 境界で replay-based tests を足すと効果が高い。
  - saved JSONL fragments / protocol events を `App` に流して expected blocks/actionbar/task state を確認する。
  - 実 socket は使わず、TUI state transition の regression を固定する。

## 実行したコマンド

```text
cargo test -p tui
```

結果:

```text
running 299 tests
test result: FAILED. 296 passed; 3 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.21s
```

失敗:

```text
multi_pod::tests::orchestrator_launch_context_uses_orchestration_root_for_runtime_workspace
spawn::tests::profile_choices_include_builtin_and_project_default_marker
spawn::tests::profile_choices_use_project_registry_default
```

```text
cargo test -p tui -- --list
```

crate の unit test inventory を確認するために使用した。

