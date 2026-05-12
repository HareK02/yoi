# Review: Submit 時 FileRef でディレクトリを参照したときの挙動

## 前提・要件の確認
- 通常ディレクトリの FileRef が `IsDirectory` Warn で黙って捨てられないこと: 満たされています。`PodFsView::resolve_file_ref` が symlink を含まない通常ディレクトリを検出し、`[Dir: <path>]` system message に展開する経路へ分岐しています（`crates/pod/src/fs_view.rs:124-143`, `crates/pod/src/fs_view.rs:240-304`）。Pod 側の alert 文脈も `[File]` / `[Dir]` 両対応に更新されています（`crates/pod/src/pod.rs:1019-1044`）。
- 採用する挙動の明確化: 満たされています。通常ディレクトリは浅い entry listing として `[Dir: <path>]` で返す仕様になっており、再帰走査やファイル本文集約には踏み込んでいません（`crates/pod/src/fs_view.rs:114-123`, `crates/pod/src/fs_view.rs:240-304`）。
- listing 上限: 満たされています。entry 件数は TUI completion と同じ `COMPLETION_LIMIT` を `DIR_FILE_REF_ENTRY_LIMIT` として使い、本文 byte 数は既存の `file_upload.max_bytes` を使う設計です（`crates/pod/src/fs_view.rs:21-25`, `crates/pod/src/fs_view.rs:278-291`, `crates/manifest/src/lib.rs:228-239`）。
- 隠しファイル・gitignore・scope 外 entry の扱い: 満たされています。hidden / gitignore は特別扱いせず、scope 上 readable な直下 entry のみを返します（`crates/pod/src/fs_view.rs:117-123`, `crates/pod/src/fs_view.rs:249-269`）。テストでも hidden / gitignore を含め、deny された entry を除外する挙動が確認されています（`crates/pod/src/fs_view.rs:456-514`）。
- symlink entry / symlink directory との整合: 満たされています。解決対象パス自体に symlink が含まれる場合は従来の `ScopedFs::read_bytes` 経路に委ね、通常ディレクトリ listing 内の symlink entry は `@` 付きで表示しています（`crates/pod/src/fs_view.rs:132-146`, `crates/pod/src/fs_view.rs:263-265`, `crates/pod/src/fs_view.rs:549-563`）。
- TUI completion とのギャップ解消: 満たされています。completion がディレクトリ候補を出す前提を維持し、submit 側でも通常ディレクトリを扱う方向に寄せています。TUI 側コメントもその仕様に更新されています（`crates/tui/src/input.rs:35-37`）。
- `Segment::FileRef` のドキュメント / コメント更新: 満たされています。Protocol 上の `Segment::FileRef` と flatten の説明が `[Dir: <path>]` listing に追従しています（`crates/protocol/src/lib.rs:127-130`, `crates/protocol/src/lib.rs:154-159`）。

## アーキテクチャ・スコープ
- FileRef 解決は既存の `PodFsView` に集約されており、Pod 本体には alert 化と attachment 組み立て以上の責務を増やしていません。層の置き方は妥当です。
- TUI completion の挙動変更ではなく、submit 側の意味論を completion に合わせる実装で、チケットの UX ギャップに対して最小限です。
- directory listing は浅い直下列挙に留まり、範囲外の再帰走査・glob 展開・ファイル本文集約には踏み込んでいません。
- `ScopedFs::read_bytes` の symlink 診断経路を温存しつつ、通常ディレクトリだけを新仕様にしているため、`file-ref-symlink-diagnostics` 側の関心と衝突していません。

## 判断
Approve — チケットで求められた通常ディレクトリ FileRef の仕様化・実装・テスト・コメント更新が揃っており、Blocking 指摘はありません。

## 確認
- `cargo fmt --check`
- `cargo test -p pod`
- `cargo test -p pod fs_view::tests::resolve_file_ref -- --nocapture`
- `cargo test -p tui input::tests -- --nocapture`
