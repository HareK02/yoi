# セッションログの Segment 保持

## 背景

`submit-segment-protocol` で `Method::Run` / `Event::UserMessage` は `Vec<Segment>` を運ぶようになり、TUI では paste を `[Clipboard #N | X chars, Y lines]` のラベルチップとして再描画できる。一方でセッション復元経路は worker の history（`Item::user_message(text)`）からしか情報を拾えず、`crates/tui/src/app.rs` の `restore_history` は user message を `vec![Segment::text(text)]` 1 個に潰して復元している。

つまり「ライブで届いた `Event::UserMessage` では paste チップが見える / 同じセッションを開き直すと潰れたテキストになる」という非対称が残っている。今後 paste 部分を context から prune する話（後続チケット）にも segment 単位の永続化が前提になるため、ここで土台を入れたい。

LLM 側の入力は flatten 済み文字列のままで良い（worker / llm-client 層は変更しない）。永続化と client 復元経路にだけ segment を残す。

## 要件

- セッションログに user message の元 `Vec<Segment>` を残す。worker の `Item` 表現や LLM 送信 payload は変更しない（`flatten_segments` の結果を引き続き食わせる）。
- 復元経路で client が segments を取り戻せるようにする。最低限、TUI の `restore_history` が paste segment を典型の magenta `[Clipboard #N | ...]` チップとして再構築できること。
- forward compat: 旧フォーマット（segments を持たないログ行）を読んでも panic / parse error にならず、従来通り text 1 個の segment として復元できること。
- 現行の compaction / fork / restore のフローを壊さない。

## 範囲外

- paste prune（segment メタデータを使って context から落とす話）。別チケットで扱う。
- FileRef / KnowledgeRef / WorkflowInvoke の resolve 結果の永続化。これらは resolver 実装チケット側の責務。
- worker / llm-client 層の API 変更。

## 完了条件

- セッションログに segment 情報が persist される（新規セッションから書かれる行は segments を含む）。
- TUI で paste を含むメッセージを送信 → セッションを開き直す → magenta チップが復元される。
- segments を持たない旧ログ行も正常に restore でき、テキスト 1 segment として表示される。
- 既存ビルド・テストを壊さない。新フォーマットに対する round-trip テスト 1 本は追加する。

## 参照

- `tickets/submit-segment-protocol.md`（前提）
- `crates/session-store/src/session_log.rs`（`LogEntry::UserInput`）
- `crates/session-store/src/session.rs`（`restore`, `RestoredState`）
- `crates/tui/src/app.rs`（`restore_history` — 現状 segment を捨てている地点）
- `crates/protocol/src/lib.rs`（`Segment`）
