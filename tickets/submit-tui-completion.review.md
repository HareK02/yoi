# Review: サブミット入力 — TUI 補完 + 型付き atom 化

レビュー対象: develop ブランチ作業ツリー（未コミット差分含む）。
スコープ: チケット `tickets/submit-tui-completion.md` の Phase 1〜4 全体。

## 前提・要件の確認

- **`@` / `#` / `/` 入力中の候補ポップアップ**: `crates/tui/src/input.rs:235-271` の `pending_completion_prefix` がトリガー検出を担い、`crates/tui/src/main.rs:397-417` で popup active 時のキーがオーバーライドされる。要件通り。
- **トリガー条件 (sigil 直前 = 行頭/空白/chip atom のみ、sigil 直後スペースで閉じる)**: `pending_completion_prefix` 内 `leading_ok` で判定し、`completion_prefix_tests` (input.rs:794-885) で 9 ケースカバー。`@src/main.rs` の `/` 誤検出回避、`@x ` での閉鎖、`foo@bar` 非トリガー等、要件のエッジを押さえている。
- **確定で indivisible chip に置換 (Backspace で塊削除)**: `replace_with_*` (input.rs:200-221) で `drain` + 単一 atom 挿入し cursor を chip 直後に置く。chip atom は `AtomClass::Chip` として既存 word motion / delete_word_before の単位扱いになっており (`atom_class`、`paste_counts_as_one_word` テスト)、Paste と同等の挙動。
- **submit 時に対応 `Segment` 変換**: `submit_segments` (input.rs:418-462) が FileRef/KnowledgeRef/WorkflowInvoke を専用 Segment に分けて出す。`knowledge_and_workflow_chips_emit_typed_segments` でカバー。
- **候補列挙の protocol query (GetHistory パターン踏襲)**: `Method::ListCompletions` / `Event::Completions` を `crates/protocol/src/lib.rs` に追加、`crates/pod/src/ipc/server.rs:91-118` で同ソケットに直接 reply、broadcast 経路なし。`event_completions_format_and_default_is_dir` / `method_list_completions_roundtrip` で wire 形を固定。
- **ファイル resolver = auto-read 切り出し**: `crates/pod/src/fs_view.rs` を新設し、旧 `compact/worker.rs` の `slice_lines` / `ReadRequirement` と旧 `pod.rs` の自動再読ロジックを `PodFsView::render_auto_read` に集約。compact 側は再エクスポート経由で利用 (compact/worker.rs:31)。`Pod::compact` も `PodFsView::new(scoped_fs.clone()).render_auto_read(...)` で同経路を通る (pod.rs:1106)。
- **Knowledge / Workflow resolver は wire のみ・空応答**: ipc/server.rs:108-110 で `CompletionKind::Knowledge | Workflow => Vec::new()` と分岐済み。
- **chip 表示**: 入力エリアは `Atom::chip()` で全 chip 共通 render (input.rs:83-92)、`Block::UserMessage` は `chip_span_for` で同色 (ui.rs:473-492)。Paste=Magenta, `@`=Cyan, `#`=Green, `/`=Yellow を input area / Block で一致。
- **Event::UserMessage の typed segment 経路**: 既存 `submit-segment-protocol.md` が完結済みで、本チケットは Atom 拡張を載せるのみ。

完了条件 4 項目すべて満たしている。`/clear` の client-side 分岐は仕様変更でチケット側から削除済み。

## アーキテクチャ・スコープ

- **`fs_view` モジュールの境界**: 「Pod から見たファイルシステム操作」という単一の責務に絞れており、auto-read と補完列挙が同居しているのは妥当 (どちらも ScopedFs + pwd + scope 上の readable 判定の薄い wrapper)。Knowledge/Workflow resolver は別スロットに将来追加される設計で、`PodFsView` に詰め込む流れにはなっていない (server.rs の kind マッチで分岐済み)。`crates/pod/src/lib.rs` 直下に置く粒度も現状の他モジュール (compact, hook, prompt) と整合。
- **`PodSharedState::fs_view: OnceLock<PodFsView>`**: 周辺は `RwLock<T>` 系だが、用途が「controller 起動時に1回 attach、以降 read-only」なので `OnceLock` の方が意図に合う。`set_fs_view` が `set` の戻り値を捨てているのは「unit test が直接生成した state にも噛み合う」コメント通りで適切。Controller 側 (controller.rs:113, 236, 289) で `fs_for_view: tools::ScopedFs;` を宣言してブロック内で代入し、ブロック後に attach する流れも冗長ではない (`fs.clone()` 1回のみ)。
- **不必要な抽象化**: 見当たらない。`FileCandidate`, `CompletionEntry` の二重定義は protocol 層と pod 内部層で意図的に分離されており (protocol は wire、pod 内部は path 構造)、`server.rs` の map で繋がっている。
- **将来用 dead code**: なし。`CompletionKind::Knowledge | Workflow` は ipc/server.rs で実際にヒットする分岐 (空応答)、`is_dir` フィールドは popup の見た目に使われている。
- **コードベースを歪めていないか**: pod 側は単純な抽出、TUI 側は既存 `Atom::Paste` の扱いをそのまま FileRef/KnowledgeRef/WorkflowInvoke に拡張する素直な拡張で、新規の概念導入は最小限。LLM provider policy / cargo add / クレート命名等の方針には影響しない。

## 指摘事項

### Non-blocking / Follow-up

- **`is_dir` 候補の確定挙動が popup の hint と乖離している** — `crates/protocol/src/lib.rs` の `CompletionEntry` doc コメント (約 357 行目付近):
  > `is_dir` is meaningful only for the file kind — it lets the TUI keep a trailing `/` after a directory selection so the user can drill in without re-typing the prefix.
  と書かれているが、`App::confirm_completion` (`crates/tui/src/app.rs:164-181`) は `entry.value` をそのまま `replace_with_file_ref` に渡すだけで、ディレクトリの場合に `/` を保持して入力継続させる経路がない。popup 側 (`ui.rs:104-110`) は `entry.is_dir` のとき表示に `/` を足しているので、ユーザーには「Tab で drill in できそう」に見えるが実際は chip として閉じる。
  対応案 (どちらも本チケット範囲外で良い):
  - doc コメントを実装に合わせて「`is_dir` は popup 表示のヒントのみ」に書き換える
  - もしくは「ディレクトリ確定時は chip にせず `@subdir/` のテキストを残してカーソルを末尾に置く」挙動を追加する
- **Paste 後に `refresh_completion()` が呼ばれない** — `crates/tui/src/main.rs:305-307` の `TermEvent::Paste(s) => app.insert_paste(s);` には refresh 呼び出しがない。`@s` の途中でクリップボード貼付した場合、popup state は古い `(kind, prefix_start, prefix)` のまま残り、次のキーストロークで refresh されるまで 1 フレーム不整合な popup が見える可能性がある。`insert_char` 系と同様に `app.refresh_completion()` を呼んで Method を返す形にしておくのが整合的。
- **`compact/worker.rs:358-364` の `slice_lines_handles_offset_and_limit` テストが `fs_view.rs:201-207` と重複** — 関数自体は `fs_view` に移動し compact 側は `use` のみなので、テストもこの移動に合わせて削除して良い (fs_view 側で同一カバレッジが取れている)。

### Nits

- `pending_completion_prefix` 内の `self.atoms.get(i.wrapping_sub(1)).filter(|_| i > 0)` (input.rs:253) は意図通り動くが、`if i == 0 { None } else { self.atoms.get(i - 1) }` の方が読みやすい。
- `controller.rs:113` の `let fs_for_view: tools::ScopedFs;` 宣言-後代入パターンは正しいが、`fs` 自身を最後に消費しているのを `fs.clone()` を `fs_for_view = fs.clone();` で先に取ってから `register_tools(tools::builtin_tools(fs, ...))` の方が見た目が単純。ただし現状でもコメントで意図が説明されており、可読性の優劣は微差。

## 判断

**Approve** — 完了条件 4 項目を満たし、アーキテクチャ・スコープともに歪みなし。フォローアップ事項はいずれも UX 補足やテスト整理で、ブロックすべきものではない。
