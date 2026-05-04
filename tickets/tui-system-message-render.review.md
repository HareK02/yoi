# Review: TUI で role:system の system message を表示する

## 前提・要件の確認

- **role:system Item を一律に表示経路に乗せる仕組み**: `Event::SystemMessage { item: serde_json::Value }` を新設し (crates/protocol/src/lib.rs:226–232)、TUI 側に `Block::SystemMessage { text }` を追加 (crates/tui/src/block.rs:22–25)。種別ごとの個別パッチではなく role:system を一律で扱える形になっている。✓
- **ライブ submit の broadcast**: `Worker::on_history_append` 汎用フック (crates/llm-worker/src/worker.rs:353–369) を新設し、`extend_history_with_callbacks` 経由で `pending_history_appends` / `ContinueWithMessages` / `PromptAction::ContinueWith` の三箇所が共通配線になった (worker.rs:889, 988, 1443)。controller.rs:259–265 で role:system だけを `Event::SystemMessage` に乗せている。`@<path>` / `/<slug>` chip 解決後の `Item::system_message` は `pending_attachments` → `PromptAction::ContinueWith(extras)` 経路で必ずこのフックを通る。✓
- **history.json を単一の情報源**: `Event::SystemMessage` の payload は `serde_json::to_value(&Item)` で `Event::History` と同じ shape。TUI は `App::push_history_item` を共通ヘルパとして抽出し (crates/tui/src/app.rs:303–423)、`handle_pod_event(Event::SystemMessage)` と `restore_history` の両方が同じ Block 生成経路に流れる。`restore_history` の `_ => {}` で握り潰していたバグも解消している (crates/tui/src/app.rs:660 周辺)。✓
- **Auto-read 含む compact 経路**: compact が新規導入する 4 種 (`[Compacted context summary]`, `[Auto-read file: ...]`, `[Session reference]`, `[Session TaskStore snapshot]`) を `compact_introduced_system_messages` にまとめ、`set_history` 直後に `broadcast_system_message_item` で個別 broadcast (crates/pod/src/pod.rs:1483–1497, 1558–1561)。retain 済みの既存 system message は再 broadcast されないことを `compact_broadcasts_only_new_system_messages_not_retained_ones` テストで担保 (crates/pod/tests/compact_events_test.rs:188–227)。✓
- **別 client が後から subscribe したケース**: `Event::History` 経路は今回 `push_history_item` に統一され、`role:"system"` を `Block::SystemMessage` として描画するブランチが追加されている (crates/tui/src/app.rs:336–339)。テスト `history_restore_renders_system_message_block` が回っている。✓
- **解決失敗時の従来経路維持**: `Alert` / user-invocation エラー側のコードは触られていない。✓
- **本文プレビュー**: `render_system_message` (crates/tui/src/ui.rs:485–550) で header 一行 + 本文 4 行 + `… (N more lines)` の素のプレビュー。Overview / Detail / Normal の 3 モードに対応。`MessageKind::System` の cyan を新設。✓

## アーキテクチャ・スコープ

- **CLAUDE.md「LLM コンテキスト加工原則」との整合**: 本実装は **history → broadcast** という許される方向のみで、context への割り込みは行わない (history を書き換えていない、新規 input を context だけに差し込んでいない)。`broadcast_system_message_item` は `set_history` 後の現在 history 内容を broadcast するだけで、worker.history と一致している。✓
- **`Worker::on_history_append` の位置づけ**: 「streaming で生成された assistant items / tool result」とは別経路で history に append される非ストリーミング項目専用のフック。assistant items (worker.rs:979)、tool_result (worker.rs:1096–1103) は意図的に通らない。役割が「streaming bypass で history に積まれる項目を上層に観測させる」と明確で、汎用化しすぎていない。✓
- **role:system フィルタの位置**: フックは Item 全種を流し、フィルタ (`is_system_message_item`) は controller 側のクロージャで掛けている。今回広がる用途 (notify_wrapper, system-reminder 系) はすべて role:system なので、フィルタ位置として妥当。✓
- **Event payload に `serde_json::Value` を使う判断**: `Event::History.items: Vec<serde_json::Value>` と同じ流儀で、TUI は同じ deser 経路を共有できる。`Item` 型をそのまま expose しなかった点は protocol crate の依存最小化として一貫している。✓
- **不要な抽象 / 歪み**: なし。新しい crate / モジュール追加もなく、既存の hook/event レーンに沿って最小拡張している。app.rs の `push_history_item` 抽出は重複排除としても妥当。

## 指摘事項

### Blocking

- なし。

### Non-blocking / Follow-up

- **Notify が二重表示になる可能性**: `Method::Notify` 受信時に `Event::Notify { message }` を即時送出 (crates/pod/src/controller.rs:480) しつつ、次ターン頭で `pending_history_appends` が `notify_wrapper` 形式の `Item::system_message` を返し、これが `Event::SystemMessage` として **追加で** broadcast される。TUI には `[notify] {message}` (Block::Notify) と `[Notification]…{message}…not a blocking request` (Block::SystemMessage) が両方並ぶ。本チケットの要件の範囲ではないが、`notify-history-persist` 系の表示設計と整合させる別チケットで整理が要る。
- **compact の broadcast 経路が二系統あるための分かりにくさ**: 通常 append は `Worker::on_history_append` フック経由、compact は `Pod::broadcast_system_message_item` 直接呼び出し、という二層になっている。理由 (compact は `set_history` の wholesale replace で意味的に差分 broadcast したい) は正当だが、将来「他にも history を再構築する操作」が増えたとき、どちらの経路に乗せるかの判断軸が暗黙のままなので、`broadcast_system_message_item` 付近にコメントで「set_history 系の wholesale replace 後に意図的に新規導入 item だけを broadcast するためのレーン」と一行残しておくと将来の読み手が楽になる。
- **`Event::SystemMessage.item` の型コメント**: protocol.rs:226–232 の docstring は良いが、payload が「Item の serialize 済み JSON (≒ history.json の 1 entry)」であることを 1 行明示すると、TUI 以外の client 実装者が deser 形式を迷わずに済む。

### Nits

- crates/tui/src/app.rs:303 `push_history_item` の `tool_call` ブランチで `name: item["name"].as_str().unwrap_or("?")` の `?` フォールバックは旧コードからの引き継ぎだが、本来 history JSON で name が欠ける状況は考えにくい。今回触ったついでに修正する性質ではないので現状維持で OK。
- crates/tui/src/ui.rs:1053 `format_pod_event` の `ScopeSubDelegated { parent_pod, sub_pod, .. }` の改行は無関係なフォーマット差分。本筋に影響なし。
- compact_events_test.rs の新テストは `worker_mut().push_item(...)` で retained system を仕込んでいるが、これは `on_history_append` を回避する経路。テストの意図 (retained は callback を通っていないので broadcast されない) を 1 行コメントで残してもよい。

## ビルド / テスト

- `cargo check --workspace --all-targets`: ✓ (既存 dead_code warning のみ、新規警告なし)
- `cargo test -p pod`: ✓ (compact_events_test の 6 件含めすべて通過、新テスト `compact_broadcasts_only_new_system_messages_not_retained_ones` も通過)
- `cargo test -p tui`: ✓ (新テスト `history_restore_renders_system_message_block`, `live_system_message_event_uses_history_item_path` 通過)
- `cargo test -p llm-worker`: ✓

## 判断

**Approve** — チケットの要件 (一般的な role:system 表示経路、ライブ / 復元 / 後着 subscribe の三経路を同じ Block バリアントに統一、compact 後の auto-read を含む 4 種を同じレーンに乗せる、解決失敗時は従来経路維持) はすべて満たされている。アーキテクチャ的にも CLAUDE.md の「許される加工」原則に整合し、既存の hook/event レーンに自然に乗る最小拡張で、コードベースを歪めていない。Notify 二重表示は本チケット範囲外で、別 ticket で整理すれば足りる。
