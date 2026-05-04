# Review: TUI に Task ストア状態を表示する

## 前提・要件の確認

### TUI 側の TaskStore 取り回し
- `App` に最新 `TaskStore` を持つ: `crates/tui/src/app.rs:84` `pub task_store: TaskStore` で保持。型は TUI 内 mirror（`crates/tui/src/task.rs`）で `tools` 依存なし。要件通り。
- `ToolCallDone` で `TaskCreate` / `TaskUpdate` を反映: `crates/tui/src/app.rs:569-586`、name は当該ブロックから取得し `task_store.apply_tool_call(name, &arguments)` を呼ぶ。完了タイミングのみで部分引数は混入しない。要件通り。
- 部分引数（streaming 中）を適用しない: `Event::ToolCallArgsDelta`（`app.rs:561-568`）は `args_stream.push_str` のみで `task_store` は触らない。要件通り。
- `SystemMessage` 観測時、`[Session TaskStore snapshot]` 始まりなら snapshot を適用: `crates/tui/src/app.rs:383-386`（`push_history_item` の "system" ブランチ）から `task_store.apply_system_message_text(&text)` を呼ぶ。`Event::SystemMessage` は `app.rs:494-497` で `push_history_item` を経由するため live 経路もカバー。要件通り。
- 初回 history replay でも同じ経路: `restore_history` が `task_store = TaskStore::new()` でリセット後（`app.rs:857-858`）、`push_history_item` を順に通すため自動復元される。要件通り。
- 不正 JSON や未知フィールドを黙って無視: `apply_tool_call` の各 match arm で `if let Ok(...)` ガード、`apply_system_message_text` も `parse_snapshot_text` が `Option` を返す。`crates/tui/src/task.rs:213-220` のテスト `malformed_arguments_are_silently_ignored` で挙動確認済み。要件通り。

### ミニビュー
- 配置 `history / mini-view / separator / status / input`: `crates/tui/src/ui.rs:70-77` の `Layout::vertical` で確認。要件通り。
- 0 件時は領域ごと畳む: `task_mini_view_height` が `is_empty()` で 0 を返し、`Constraint::Length(0)` で行ごと消える（`ui.rs:99-106`）。要件通り。
- 最大 3 件 active を表示 + サマリ 1 行: `MINI_VIEW_MAX_ACTIVE = 3`（`ui.rs:94`）と `mini_view_summary_line` で実装。subject の改行は `entry.subject.lines().next()` で先頭行のみ（`ui.rs:141`）。要件通り。
- 件数サマリ: pending / inprogress / completed / deleted の内訳を1行で表示（`ui.rs:153-167`）。要件通り。

### サイドペイン
- トグルキー `Ctrl-T`: `crates/tui/src/main.rs:436-439`。要件通り。
- 横分割は history 領域内のみ: `draw_history` 内で `Layout::horizontal` 分割し、status / input / separator は外側 `chunks` の全幅を維持（`ui.rs:326-334`）。要件通り。
- 全件（pending / inprogress / completed / deleted）+ description を含めて列挙: `draw_task_side_pane`（`ui.rs:386-460`）。各エントリは `taskid` + status mark + subject + description を表示。要件通り。
- スクロール対応: `task_pane_scroll` を `App` に保持、`PageUp` / `PageDown` がペイン open 時に優先（`main.rs:471-489`）。最大値で clamp（`ui.rs:449-452`）。要件通り。

### レイアウトと一貫性
- ミニビューとサイドペインは同じ `App::task_store` を参照する単一情報源（`ui.rs:81, 399`）。要件通り。
- 既存の completion popup は `chunks[4]` (input rect) を使うため、ミニビューが挟まっても座標計算に影響なし（`ui.rs:86-88, 191-246`）。
- スクロール / status line は `chunks[0]` の `inner` を使うため、サイドペインが開いてもスクロール計算は分割後の history rect に追従（`ui.rs:347-350`）。
- 副作用無し。

### 完了条件
すべて満たしている。`cargo test -p tui` 71 件合格、`cargo build --workspace` クリーン。

## アーキテクチャ・スコープ

- **`tools` 依存を持ち込まない方針** が一貫して守られている。TUI 内 `task.rs` mirror は `tools::TaskStore` 全機能ではなく TUI が必要とする最小サブセット（`apply_tool_call` / `apply_system_message_text` / `tasks()` / `counts()`）に絞っている。`feedback_llm_worker_scope` / 「TUI は表示専門レイヤ」の方針に整合。
- `serde` は `cargo add` で追加（`crates/tui/Cargo.toml:19`）。memory の `feedback_cargo_add` 遵守。
- protocol 表面は無変更。`Event::SystemMessage` / `Event::ToolCallDone` / `Event::History` の既存経路にすべて乗っている。チケットの「protocol 表面を増やさない」目標を達成。
- LLM コンテキスト加工原則: history からの純粋な再構成変換であり context への割り込みではない。整合済み。
- ファイル分割: 新規 `task.rs` モジュール（180行 + テスト）として切り出されており、`app.rs` / `ui.rs` への変更も既存パターン（cache.rs / scroll.rs と同じ位置付け）に整合。コードベースを歪めていない。

## 指摘事項

### Non-blocking / Follow-up
- **snapshot text フォーマット契約の自動検出機構が無い**: `crates/tui/src/task.rs:168-179` `parse_snapshot_text` は `crates/tools/src/task.rs:393-404` `parse_compact_snapshot_text` と byte-for-byte 同一の手書きクローン。`tools::render_snapshot` のフォーマット（フェンス記号・改行配置・ヘッダ文字列）が変わると、TUI は無言で snapshot 検出を失敗するだけで CI からは見えない。Doc コメント（`task.rs:8-17`）と TUI 側ユニットテストの手書き fixture（`wrap_snapshot` ヘルパ）で「契約に乗っているつもり」を表明しているが、`tools::render_snapshot` の出力をそのまま食わせる意味でのリンク確認は無い。
  - 落としどころ案: `crates/tools` を `dev-dependencies` だけに入れ、`#[cfg(test)]` で `tools::render_snapshot(...)` の実出力を `tui::task::TaskStore::apply_system_message_text` に流すクロステストを 1 本足す。本番ビルドの依存は増やさず、契約破綻だけ拾える。今のスコープ外でも構わない。
- **History replay 経路に「retained TaskCreate → 末尾 snapshot で上書き」シナリオの App テストが無い**: `tools` 側には `trailing_snapshot_supersedes_pre_compact_taskcreates_in_retained`（`crates/tools/src/task.rs:584-628`）があるが、TUI の `App::handle_pod_event(Event::History { ... })` 経由で同じ並びを通す App レベルテストは未追加。実装上は `push_history_item` を順に呼ぶだけなので動くはずで、`history_replay_reconstructs_task_store`（`app.rs:1404-1448`）と `snapshot_text_replaces_state_and_advances_next_id`（`task.rs:251-284`）の組み合わせで間接的にカバーはされている。リグレッション保護として 1 本欲しい程度の話。
- **狭い端末でのトグル無反応**: `task_side_pane_width`（`ui.rs:373-384`）は `area_width < 60` なら 0 を返すため、`task_pane_open == true` でも視覚的に何も起きない。Ctrl-T が "効いていない" ように見えるリスクがある。レアケースなので非ブロッキング。

### Nits
- `crates/tui/src/task.rs:165-166` の `TaskSnapshot` は `tasks` フィールドのみだが、本家 `tools::TaskSnapshot` には他フィールドが追加される可能性がある。`#[serde(deny_unknown_fields)]` を付けないことで forward-compat を保っている（適切）が、その意図を 1 行コメントしておくと将来読み手に親切。
- `task_status_mark`（`ui.rs:172-184`）の戻り値タプルに型エイリアスを入れると、サイドペイン側でも mini-view 側でも使われている共有意図がコードから読みやすい。今のままで実害は無い。

## 判断

**Approve** — チケットの前提・方針・要件・完了条件をすべて満たしており、設計の意図（`tools` 非依存・protocol 不変・history からの純粋再構成）も忠実に実装されている。tools 側との snapshot フォーマット契約はテキスト一致でしか担保されていないが、これはチケットが意識的に選んだトレードオフであり、必要であれば dev-dependency でクロステストを 1 本足す程度のフォローアップで十分。コードベースを歪める箇所は無い。
