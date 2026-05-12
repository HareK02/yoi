# Submit 時 FileRef でディレクトリを参照したときの挙動

## 背景

submit 時に `Segment::FileRef { path }` は `crates/pod/src/fs_view.rs` の
`PodFsView::resolve_file_ref` でファイル本文として読まれ、`[File: <path>]` system
message に展開される。内部では `ScopedFs::read_bytes` を経由するため、`path` が
ディレクトリだった場合は `ToolsError::IsDirectory` で失敗し、Pod 側は
`AlertLevel::Warn` を出してそのセグメントを丸ごと捨てる
（`crates/pod/src/pod.rs` の `resolve_file_refs`）。

ユーザーから見ると `@some-dir` を添付しても LLM 側にはプレースホルダが残る
だけで、ディレクトリ参照という意図が反映されない。TUI completion は
ディレクトリも候補として出すので、実際の挙動とのギャップが大きい。

`tickets/file-ref-symlink-diagnostics.md` は symlink が canonicalize 後に
ディレクトリ・scope 外だった場合の診断にフォーカスしており、通常ディレクトリの
FileRef 解決をどう扱うかは扱っていない。

## ゴール

submit 時に `Segment::FileRef` がディレクトリを指している場合の挙動を
仕様として確定し、ユーザーが添付の意図どおりの結果を LLM に届けられる
ようにする。

## 要件

ディレクトリに対する FileRef の意味論を決め、実装する。少なくとも以下の
論点をチケット内で結論づけ、実装・テスト・ドキュメントを揃える:

- 採用する挙動: 浅い entry listing を `[Dir: <path>]` 等の label で返すか、
  明示的に reject して `read_file` / `glob` の利用を促すか、それ以外か
- listing を返す場合の上限: entry 件数や本文 byte 数を、ファイル本文用の
  upload/attachment 上限と同じにするか別途持つか
- 隠しファイル・gitignore・scope 外 entry の扱い
- symlink entry の扱いは `tickets/file-ref-symlink-diagnostics.md` と矛盾
  しないこと（重複する判定は共通化を検討する）
- TUI completion がディレクトリ候補を出す挙動と整合すること（候補に出る
  以上、submit 時に黙って捨てるのは UX として不整合）

## 完了条件

- 通常ディレクトリの FileRef が、`IsDirectory` Warn で黙って捨てられる
  現状の挙動を残していない
- 採用した挙動が `PodFsView` のテストで覆われている
- `Segment::FileRef` のドキュメント / コメントが新仕様に揃っている
- TUI completion とのギャップが解消されている（ディレクトリを候補から
  外すのか、submit でも扱うのか、いずれかに寄せる）

## 範囲外

- 再帰的なディレクトリ走査、glob 展開、`tree` 風の深い表現
- ディレクトリ内ファイルの自動 read 集約（auto-read / fs view 側の責務）
- symlink 経由のディレクトリ判定の刷新
  （`tickets/file-ref-symlink-diagnostics.md` 側で扱う）
- upload / attachment 上限の manifest 化（`tickets/manifest-output-upload-limits.md`）

## 参照

- `crates/pod/src/fs_view.rs` `resolve_file_ref` / `list_file_completions`
- `crates/pod/src/pod.rs` `resolve_file_refs`
- `crates/tools/src/scoped_fs.rs` `read_bytes`
- `tickets/file-ref-symlink-diagnostics.md`
- `tickets/manifest-output-upload-limits.md`

## Review
- 状態: Approve
- レビュー詳細: [./file-ref-directory.review.md](./file-ref-directory.review.md)
- 日付: 2026-05-12
