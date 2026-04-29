# セッションログの Segment 保持

## 背景

`submit-segment-protocol` で `Method::Run` / `Event::UserMessage` は `Vec<Segment>` を運ぶようになり、TUI では paste を `[Clipboard #N | X chars, Y lines]` のラベルチップとして再描画できる。一方でセッション復元経路は worker の history（`Item::user_message(text)`）からしか情報を拾えず、`crates/tui/src/app.rs` の `restore_history` は user message を `vec![Segment::text(text)]` 1 個に潰して復元している。

つまり「ライブで届いた `Event::UserMessage` では paste チップが見える / 同じセッションを開き直すと潰れたテキストになる」という非対称が残っている。今後 paste 部分を context から prune する話（後続チケット）にも segment 単位の永続化が前提になるため、ここで土台を入れたい。

LLM 側の入力は flatten 済み文字列のままで良い（worker / llm-client 層は変更しない）。永続化と client 復元経路にだけ segment を残す。

## 前提チケット

- `tickets/session-log-decouple-item.md` — session-store が llm-worker `Item` から独立した永続スキーマを持つようにする。本チケットはその上で `UserInput` を `Item` から `Vec<Segment>` に置き換える。

## 要件

- セッションログに user message を `Vec<Segment>` として残す。worker の `Item` 表現や LLM 送信 payload は変更しない（`flatten_segments` の結果を引き続き食わせる）。
- `LogEntry::UserInput` から `item: Item` を取り除き、`segments: Vec<Segment>` のみ持つ形に置き換える。replay 時には `flatten_segments` で 1 文字列にして `Item::user_message(text)` を派生させ worker history に積む。
- 復元経路で client が segments を取り戻せるようにする。最低限、TUI の `restore_history` が paste segment を典型の magenta `[Clipboard #N | ...]` チップとして再構築できること。
- 後方互換は持たない。既存 jsonl ログは捨てる前提。
- 現行の compaction / fork / restore のフローを壊さない。

### Pod と save_delta の責務分割

`save_delta` は worker history の差分を分類して `LogEntry::UserInput` を書いていたが、segments は worker history に存在しない。Pod 側で `Method::Run` 入口直後（`flatten_segments` 直前）に segments で `LogEntry::UserInput` を直接書き、`save_delta` からは user_message 分類を外す（assistant / tool_result / hook_injected のみ扱う）。

### Event::History への segments 載せ方

Pod が吐く `Event::History.items: Vec<serde_json::Value>` の user message オブジェクトに `segments` フィールドを embed する（既存の混合 JSON 表現を活かす）。TUI 側 `restore_history` は user 分岐で `segments` を読み出して `Block::UserMessage { segments }` を組む。

## 範囲外

- paste prune（segment メタデータを使って context から落とす話）。別チケットで扱う。
- FileRef / KnowledgeRef / WorkflowInvoke の resolve 結果の永続化。これらは resolver 実装チケット側の責務。
- worker / llm-client 層の API 変更。

## 完了条件

- セッションログに segment 情報が persist される（`LogEntry::UserInput` が `{ ts, segments }` 形）。
- TUI で paste を含むメッセージを送信 → セッションを開き直す → magenta チップが復元される。
- 既存ビルド・テストが新スキーマで合格。
- segments → flatten → Item の派生経路を round-trip テストで verify する。

## 参照

- 前提: `tickets/session-log-decouple-item.md`
- `crates/session-store/src/session_log.rs`（`LogEntry::UserInput`）
- `crates/session-store/src/session.rs`（`save_delta`, `restore`, `RestoredState`）
- `crates/pod/src/pod.rs`（`run`, `flatten_segments`, `persist_turn`）
- `crates/pod/src/controller.rs`（`Event::UserMessage` broadcast 経路）
- `crates/tui/src/app.rs`（`restore_history` — 現状 segment を捨てている地点）
- `crates/protocol/src/lib.rs`（`Segment`, `Event::History`）
