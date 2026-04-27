# Review: サブミット入力 protocol Segment 化

## 前提・要件の確認

### protocol
- `Method::Run` と `Event::UserMessage` が `Vec<Segment>` で wire を通る:
  - `crates/protocol/src/lib.rs:14` (`Run { input: Vec<Segment> }`) / `:162` (`UserMessage { segments: Vec<Segment> }`)。`tag = "kind"` の internally-tagged enum で Text/Paste/FileRef/KnowledgeRef/WorkflowInvoke/Unknown を定義 (`:97-125`)。完了。
- 全 5 variant 定義: 完了 (`:101-119`)。
- forward compatibility: `#[serde(other)]` で `Segment::Unknown` に吸収 (`:123`)。専用テスト 2 本 (`:416-438`) で deserialize 成功と `Method::Run` 内での共存を確認。完了。
- dumb client 用ヘルパー: `Segment::text` (`:129`) と `Method::run_text` (`:138`) を用意し、ドキュメントコメントで「`vec![Segment::Text(_)]` のみで動く」前提を明記 (`:84-96`)。完了。
- `Event::UserMessage roundtrip` の更新版テスト (`:749-770`) も追加済み。

### Pod 側 resolve
- `Pod::run` が `Vec<Segment>` を受け、`flatten_segments` で単一文字列に展開 (`crates/pod/src/pod.rs:580-655`)。Text/Paste は inline、FileRef/KnowledgeRef/WorkflowInvoke/Unknown は `[unresolved <kind>: <key>]` プレースホルダに置換し同時に `Alert(Warn, Pod, …)` を発火。要件の **2 経路同時通知** を満たす。
- `Pod::run_text` shim (`:561-566`) と `Method::run_text` の対応関係も整合。
- `interrupt_and_run` も `Vec<Segment>` シグネチャに揃え、内部で `self.run(input)` に委譲 (`crates/pod/src/interrupt_and_run.rs:27-47`)。
- Controller 側で `Method::Run { input }` を受けると `Event::UserMessage { segments: input.clone() }` を broadcast し、その後 `pod.run(input)` / `interrupt_and_run(input)` に渡す (`crates/pod/src/controller.rs:273-299`)。Event 経路は typed のまま再放送される。

### TUI 側
- `InputBuffer::submit_segments` が `Atom::Char` を 1 つの `Segment::Text` に collapse、`Atom::Paste` を独立した `Segment::Paste` に分離 (`crates/tui/src/input.rs:198-221`)。テスト 4 本 (`:428-490`) で純テキスト・前後 Text に挟まれた Paste・空入力・先頭 Paste のケースをカバー。
- `submit_input` は `submit_segments()` を経由して `Method::Run { input: segments }` を送出 (`crates/tui/src/app.rs:64-81`)。空入力 (`segments_are_blank`, `:497-502`) の Pause/Resume 動作も保たれている。
- `Block::UserMessage` が `segments: Vec<Segment>` に置き換わり (`crates/tui/src/block.rs:17-19`)、`render_user_message` (`crates/tui/src/ui.rs:368-420`) が paste セグメントを magenta `[Clipboard #N | X chars, Y lines]` で再構築、Overview モードは `segment_display_text` で one-liner にする (`:425-439`)。Unknown variant は `[unknown segment]` 表示。
- `Event::UserMessage { segments }` ハンドラが typed を直接 `Block::UserMessage` に積む (`crates/tui/src/app.rs:93-100`)。
- `restore_history` の user message 側はテキストのみを `vec![Segment::text(text)]` でラップ (`:373-376`) — 後述の non-blocking 指摘あり。

### unknown variant / 未登録 resolver
- `flatten_segments` が unknown / FileRef / KnowledgeRef / WorkflowInvoke すべてに対し placeholder + `Alert` を発行 (`crates/pod/src/pod.rs:609-651`)。決定済みルール通り。
- 統合テスト `run_with_unresolved_segment_emits_alert_and_placeholder` (`crates/pod/tests/controller_test.rs:417-467`) が `FileRef` ケースで Alert と placeholder の両発火を end-to-end で立証。
- `run_with_paste_segment_inlines_content_and_emits_typed_user_message` (`:357-415`) が paste 経路の hybrid 性質 (LLM には inline 本文・Event::UserMessage には typed segments) を立証し、ラベルの LLM への漏洩がないことも明示的に assert (`:414`)。

### ライフサイクル/ビルド条件
- 既存テスト群は `Method::run_text` / `Pod::run_text` への置換で sweep 済み (`crates/pod/examples/*.rs`、`crates/pod/tests/*.rs` 全般)。残存する直接 `Method::Run { input: ... }` は (a) submit_input の本流、(b) 新統合テスト、(c) FFI 側のアサート (`spawn_pod_test.rs:196`、`pod_comm_tools_test.rs:188`) のみで、いずれも Vec<Segment> 形に揃っている。
- `Worker::run(String)` 自体は LLM-worker 層の低レベル API なので変更しない判断は妥当。Pod が flatten 一回で接続する単一経路 (要件) と整合。
- ビルド・テストは緑で、警告は事前から存在する `llm-worker/timeline.rs` の `end_scope` のみ — 本チケットによる退行なし。

## アーキテクチャ・スコープ

- レイヤ境界: `Segment` / placeholder / Alert の生成は **Pod 層** に閉じており、`llm-worker` には漏れていない (Worker は引き続き String を受け取る)。`MEMORY.md` の「llm-worker は低レベル基盤に留める」方針を守れている。
- TUI 側は `submit_segments` が新責務として追加されただけで、parser や resolver は持ち込まれていない。submit-tui-completion で扱う `@`/`#`/`/` 補完は範囲外、適切に分離されている。
- 新規の resolver trait は導入されておらず、要件通り「variant 定義 + 未登録時フォールバック」で着地している。後続チケット (memory / workflow) のための余分な抽象化なし。
- `flatten_segments` を `Pod::run` から呼ぶ形に閉じ込めた点も妥当: 本文展開ロジックを 1 箇所に集中。
- 新依存の追加なし、新クレートの追加なし。`alerter.rs` / `notify_buffer.rs` は同チケットの notification-naming-cleanup のリネームを取り込んだ既存リファクタの一部であり、本チケットの範囲とは独立して整理されている (本チケット由来ではない)。

## 指摘事項

### Blocking
なし。

### Non-blocking / Follow-up
- **session log replay は paste チップを失う**: 要件本文の「session log / Event::UserMessage 上ではラベル化情報を保持」のうち、Event 経路は満たしているが session log は inlined テキストのみを保持し、`restore_history` で `vec![Segment::text(text)]` に潰れる (`crates/tui/src/app.rs:373-376`)。完了条件の方では「Event 経由の再描画では `[Clipboard #N | ...]` が復元される」と Event 経路に絞られているため本チケットでは合格判断としたが、後で GUI / 別クライアントが履歴 fetch する場面で paste 識別が失われる。完全に保持するには Worker history か session_store に typed segments を別途保存する必要があるため、後続チケット (例えば native-gui-mvp や memory 関連) で「session log にも segment metadata を残すか」を扱うのがよい。
- **`Segment::Unknown` の end-to-end 統合テストがない**: protocol レベルの deserialize テストはあるが、Pod 統合テストでは FileRef だけが Alert + placeholder の両発火を確認している。Unknown は同一の `flatten_segments` 分岐を通るので回帰耐性は十分だが、forward-compat の信頼の証として 1 ケース足してもよい。
- **`Event::UserMessage` 経由の paste チップ復元は live subscriber のみが受け取れる**: 後発接続クライアントは `GetHistory` で取れるのが Worker の `Item::Message`（flatten 済み文字列）だけ。上記 1 点目と同じ根の話。

### Nits
- `Segment::Unknown` を再シリアライズすると `{"kind":"unknown"}` になり情報損失するが、本チケットでは forward-compat の片方向だけで十分という整理 (`crates/protocol/src/lib.rs:121-122` のコメントに明記済み)。意図通り。
- `flatten_segments` の placeholder 文言 (`[unresolved file ref: …]` 等) は LLM 側プロンプトの一部になる。将来 prompt catalog に逃がすかは別途検討すべきかも (現時点では英語固定で OK)。
- `crates/tui/src/ui.rs:404` で `lines: line_count` と `lines` フィールドのシャドウイングを避けるため `line_count` にリネームしているのは適切。

## 判断
**Approve** — チケットの要件と完了条件はすべて満たされており、forward-compat と hybrid フォールバックの両経路が end-to-end のテストで立証されている。session log 側の paste チップ保持は要件本文に言及があるものの完了条件は Event 経路に絞られているため、本チケット範囲外として後続チケットでフォローすればよい。コードベースを歪める方向の追加抽象化は持ち込まれておらず、Pod / TUI / protocol の責務分離も保たれている。
