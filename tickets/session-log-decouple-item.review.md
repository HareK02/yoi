# Review: Session log の Item 依存を切り離す

## 前提・要件の確認

- **session-store crate が `LoggedItem` 系の独自型を export し、`Item` への依存が UserInput を除いて消える**
  満たされている。`crates/session-store/src/logged_item.rs` に `LoggedItem` / `LoggedRole` / `LoggedContentPart` を新設し、`crates/session-store/src/lib.rs:31-43` で公開。`LogEntry` の `AssistantItems` / `ToolResults` / `HookInjectedItems` / `SessionStart.history` は `Vec<LoggedItem>` に置換済み（`session_log.rs:106,123,126,129`）。`UserInput` のみ後続チケットの責務として `Item` 参照が残るが、本チケットでは触らない方針通り。
- **`collect_state` で組まれた `RestoredState.history` が従来と同じ Item 列を返す**
  満たされている。`session_log.rs:252,262,267,272` で `Item::from(LoggedItem)` を介して再構築。既存の worker / pod テスト（`cargo test --workspace` 全合格）と置換テスト群で確認済み。
- **`save_delta` の外側 API は変えず、内部で Item → LoggedItem 変換を通す**
  満たされている。`session.rs:178` のシグネチャ `&[Item]` は不変、`to_logged()` 経由で永続化（`session.rs:202,221,232`）。
- **session-store の単体テストを新スキーマに合わせて書き換え**
  満たされている。`fs_store_test.rs` / `session_test.rs` を `.into()` 経由に更新。
- **Round-trip テスト 1 本追加**
  超過達成。`logged_item.rs` 内に 5 本の round-trip テスト（user_message / tool_call / reasoning+ZDR / tool_result_with_content / kind タグ確認）を追加。
- **既存ビルド・全テストが新スキーマで合格**
  満たされている。`cargo test --workspace` 全合格。

## アーキテクチャ・スコープ

- 永続スキーマと worker 内部型の分離を logged_item モジュールで完結させている。From/Into 変換は session-store の責務として閉じており、pod / worker から見れば save_delta の API は不変。レイヤ違反なし。
- LLM ZDR の制約（Reasoning::encrypted_content の保持）が頭注（`logged_item.rs:11-13`）に明記され、テスト（`round_trip_reasoning_preserves_encrypted_content`）でも担保されている。「replay に必要な field のみ持つ」という方針判断の妥当性を担保する記述として適切。
- `id: ItemId` / `status: ItemStatus` を意図的に落とし、replay 時に `None` で synthesize する方針はチケット記載と一致。output-side metadata と replay-side metadata の分離が綺麗に効いている。
- `LogEntry::Extension` などの既存ログ拡張点に手を入れていない点も適切（スコープ厳守）。
- session-store の `lib.rs` で `llm_worker::{Item, ContentPart, Role}` を再 export しているが、これは `RestoredState.history: Vec<Item>` を消費する側のために残している既存挙動の維持で、ticket の趣旨（`Item` 依存を「ログスキーマから」剥がす）には反しない。

## 指摘事項

### Non-blocking / Follow-up

- `lib.rs:45` の `pub use llm_worker::llm_client::types::{ContentPart, Item, Role};` は外部利用箇所が見当たらない（`grep` 結果）。dead 再エクスポートは将来のクリーンアップ余地としてメモ。本チケットの範疇では削除不要。

### Nits

- `from_logged` は `pub` で公開されているが、現状 `collect_state` 経由の `iter().cloned().map(Into::into)` パターンしか使われていない。slice 版が必要になるまで public API として残す価値があるかは要観察。

## 判断

**Approve** — チケットの完了条件はすべて満たされ、round-trip テストも要件以上に厚く配置されている。スキーマ分離の方針通り Item 依存は UserInput を除いて消えた。後続 `session-log-segments` の前提が綺麗に整っている。
