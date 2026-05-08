# Review: FileRef / file tools の symlink 診断と外部参照導線

## 前提・要件の確認

- broken symlink / scope 外 symlink / symlink directory / file-vs-directory の診断: 概ね満たされている。`ToolsError` に `BrokenSymlink`、`SymlinkOutOfScope`、`SymlinkTargetIsDirectory`、`SymlinkDirectoryNotTraversed` が追加され、元 path と target を含むエラー文になっている（`crates/tools/src/error.rs:19-58`）。
- scope safety: 満たされている。`ScopedFs::read_bytes` / `write` は引き続き `scope.is_readable` / `scope.is_writable` を通し、symlink target が scope 外の場合は許可せず診断へ落としている（`crates/tools/src/scoped_fs.rs:108-141`, `crates/tools/src/scoped_fs.rs:160-226`）。既存 edge-case test でも外部 target が書き換わらないことを確認している（`crates/tools/tests/edge_cases.rs:83-158`）。
- scope 内 symlink file / directory の仕様: 満たされている。scope 内 symlink file は読み取り可能、既存 symlink file への write は symlink を置換せず canonical target を更新する仕様としてテストされている（`crates/tools/src/scoped_fs.rs:435-543`）。symlink directory root は Glob/Grep で辿らず明示エラーにする実装になっている（`crates/tools/src/glob.rs:97-147`, `crates/tools/src/grep.rs:254-304`）。
- file tool のテスト: 満たされている。`ScopedFs` unit test と `edge_cases` で broken / out-of-scope / in-scope file / directory type がカバーされている（`crates/tools/src/scoped_fs.rs:389-543`, `crates/tools/tests/edge_cases.rs:83-158`）。
- 外部参照導線: 満たされている。外部 clone を読む場合は実体 path を read scope に入れること、workspace symlink だけでは権限が増えないこと、Glob/Grep は symlink directory を辿らないことが文書化された（`docs/file-ref-symlinks.md:1-12`）。

## アーキテクチャ・スコープ

- 変更は `tools` crate の scope-aware filesystem と Glob/Grep surface に閉じており、Pod / protocol / manifest の権限モデル自体には踏み込んでいない。チケットの範囲外である scope 自動追加や symlink の無条件許可も入っていない。
- symlink 診断は `ScopedFs` の helper と `ToolsError` に集約されており、既存の `Scope` 判定を迂回していない。scope escape safety を保ったまま診断を改善する方向として妥当。
- write で既存 symlink file の canonical target を更新する変更は、従来の atomic tempfile persist が symlink 自体を置換し得る挙動より安全で、今回の目的にも合っている。

## 指摘事項

### Non-blocking / Follow-up

- `Grep` でも symlink directory root の明示エラーが実装されているが、専用テストは `Glob` 側のみ確認できる（`crates/tools/src/grep.rs:298-304`, `crates/tools/src/glob.rs:344-372`）。挙動は単純で `cargo test -p tools` も通っているため blocking ではないが、将来の退行防止として `grep_reports_scope_inside_symlink_directory_is_not_traversed` 相当のテストを追加してもよい。
- nested symlink directory は walker の `follow_links(false)` により従来通りスキップされ、skip ごとの診断は出ない。実装 Pod の報告通り、per-entry diagnostics は結果形式の拡張が必要なので本チケットでは妥当な見送り。

## 判断

Approve with follow-up — 要件は満たされ、scope safety も維持されている。Grep の専用テスト追加と nested symlink directory 診断は follow-up として扱えばよい。

## 確認したコマンド

- `cargo test -p tools`
- `cargo fmt -p tools --check`
