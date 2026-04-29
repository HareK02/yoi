# Session log の Item 依存を切り離す

## 背景

`crates/session-store` の `LogEntry` は、history 構成パートをすべて llm-worker の `Item` enum でそのままシリアライズしている:

- `UserInput.item: Item`
- `AssistantItems.items: Vec<Item>`
- `ToolResults.items: Vec<Item>`
- `HookInjectedItems.items: Vec<Item>`
- `SessionStart.history: Vec<Item>`

これは worker 内部型と永続フォーマットを結合しているため、`Item` / `ContentPart` / `Reasoning` のフィールド追加・名称変更が即ログ非互換になる。永続データは llm-worker の進化と独立に安定したスキーマを持つべき。

`tickets/session-log-segments.md`（user message を `Vec<Segment>` で残す）の前提として、まず session-store が自分のスキーマを持つように剥がす。

後方互換は持たない（既存 jsonl は捨てる）。新スキーマで一新する。

## 方針

### session-store に独自の logged 型を置く

llm-worker の `Item` をそのまま流用せず、session-store 内に永続用の型を切り出す:

```rust
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LoggedItem {
    Message    { role: LoggedRole, content: Vec<LoggedContentPart>, .. },
    ToolCall   { call_id, name, arguments, .. },
    ToolResult { call_id, summary, content, .. },
    Reasoning  { text, summary, encrypted_content, .. },
}
```

具体形は実装で決める。要件:

- replay に必要な field のみ持つ（`ItemId` や `ItemStatus` のように LLM 送信 / round-trip に効かないものは原則持たない。ただし ZDR 用 `encrypted_content` のような stateless 再送に必要なものは保持する）
- worker `Item` のフィールド追加でログ非互換を起こさない
- snake_case JSON、既存 LogEntry の他フィールドと整合

### 変換器

- `From<&Item> for LoggedItem`（worker → logged、save 経路）
- `LoggedItem::into_item()`（logged → worker、replay 経路）
- session-store crate 内の責務。worker / pod は変換を意識しない

worker → logged の変換で落ちる field が出るのは構わない（永続化に不要なら捨てる）が、replay → 再 save で wire-equivalent な Item が再生される構造にする。

### LogEntry の差し替え

- `AssistantItems.items` / `ToolResults.items` / `HookInjectedItems.items` / `SessionStart.history` を `Vec<LoggedItem>` に置換
- `collect_state` は logged → Item 変換を通して `RestoredState.history` を組む
- `save_delta` は Item → logged 変換を通して書き込む

`UserInput.item` は触らない。直後の `session-log-segments` で segments に置き換わるため、ここで logged 化しても 1 ステップで再変更になる。

### ログの hash chain

`compute_hash` は `LogEntry` を JSON シリアライズして SHA-256 を取る。スキーマが変わるのでハッシュ値は別物になる。新規セッションから新スキーマで書き始める前提（既存ログを読まないので問題ない）。

## 範囲外

- 後方互換（既存 jsonl ログの読み込み）
- `UserInput.item` の差し替え（`session-log-segments` で対応）
- ログフォーマットのバージョニング機構 — 必要になったら追加
- llm-worker の `Item` 構造変更
- compaction / fork / restore 経路自体の再設計

## 完了条件

- session-store crate が `LoggedItem` 系の独自型を export し、`Item` への依存が UserInput を除いて消える
- `collect_state` で組まれた `RestoredState.history` が従来と同じ Item 列を返す（既存の worker / pod テストが通る）
- `save_delta` の外側 API は変えず、内部で Item → LoggedItem 変換を通す
- session-store の単体テストを新スキーマに合わせて書き換え、すべて合格
- round-trip テストを 1 本追加: `Item → LoggedItem → JSON → LoggedItem → Item` で意味的に等価
- 既存のビルド・全テストが新スキーマで合格

## 参照

- 後続: `tickets/session-log-segments.md`
- 影響範囲: `crates/session-store/src/session_log.rs`, `crates/session-store/src/session.rs`, `crates/session-store/tests/*`
- 不変: `crates/llm-worker/src/llm_client/types.rs`（`Item` / `ContentPart` 等）、`crates/pod`（`save_delta` の呼び出し側）
