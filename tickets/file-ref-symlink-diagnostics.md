# FileRef / file tools の symlink 診断と外部参照導線

## 背景

insomnia 開発では、外部の参考プロジェクトを `external checkout` のローカル clone として参照する運用がある。今回、workspace 内へ外部 clone を指す symlink を置き、`@tmp/<external-project>` として読ませようとしたが、参照側からは期待通りに読めなかった。

このような symlink は、壊れている場合、相対リンクの基準が利用者の期待と違う場合、または canonicalize 後の target が Pod の readable scope 外に出る場合がある。現状は `file not found` のように見えやすく、壊れた symlink なのか、scope 外なのか、Glob / FileRef が symlink directory を辿らない仕様なのか判断しにくい。

一方で、symlink を無条件に辿ると workspace scope の外へ読み書きできる escape になるため、単純に許可してはいけない。canonicalized target が effective scope 内かどうかを明確に検査し、失敗時は理由を出す必要がある。

## 要件

### symlink 診断

FileRef / completion / read 系の file handling で、対象 path または途中 component が symlink の場合に、失敗理由を区別して報告できるようにする。

最低限、以下を区別する。

- symlink target が存在しない（broken symlink）
- symlink target が Pod の readable scope 外に解決される
- target は scope 内だが directory traversal / glob 側が symlink directory を辿っていない
- target は scope 内だが file / directory 種別が期待と違う

エラー文には、可能な範囲で元 path と解決後 target を含める。

### scope safety

symlink は canonicalize 後の target で scope 判定する。

- read: canonicalized target が readable scope 内の場合のみ許可
- write: canonicalized target が writable scope 内の場合のみ許可
- scope 外の場合は拒否し、`read scope に target を追加する` / `workspace 内へコピーする` / `正しい symlink を作り直す` などの対処を示す

既存の symlink escape 防止テストは維持する。

### 外部 external checkout 参照の導線

外部参考プロジェクトを `external checkout` から読む運用に対して、推奨手順を明文化する。

候補:

- Pod 起動時 / spawn 時に external checkout clone の実体 path を read scope に入れる
- workspace 内 symlink を使う場合も、target が readable scope 内に入っている必要があることを明記する
- broken relative symlink を検出した場合、絶対 symlink または正しい相対 symlink を促す

### 対象 surface

少なくとも以下のいずれか、または共通層を通じて同等の挙動になること。

- submit 時 `Segment::FileRef` の resolver
- TUI completion の file candidate 表示
- generic file read / glob 系 tool
- auto-read / fs view が同じ file handling を使う場合はその経路

## 範囲外

- readable scope 外 symlink を安全確認なしに自動許可すること
- symlink target を自動で scope に追加すること
- external checkout clone の自動探索や自動 clone
- write scope の owner handoff / Pod sleep など権限モデル自体の変更
- Git submodule / worktree の設計変更

## 完了条件

- broken symlink、scope 外 symlink、scope 内 symlink directory の挙動が区別できる
- scope 外 symlink は canonicalized target で拒否され、明確なエラーになる
- scope 内 symlink file / directory は、仕様として許可するか拒否するかが明文化され、テストされている
- FileRef または file tool のテストで symlink 経路がカバーされる
- external checkout など外部参照プロジェクトを読む場合の推奨手順が docs または ticket 内に記録されている

## 参照

- `crates/tools/src/scoped_fs.rs`
- `crates/tools/tests/edge_cases.rs`
- `crates/pod/src/pod.rs` `resolve_file_refs`
- `crates/pod/src/fs_view.rs`
- `crates/manifest/src/scope.rs`

## Review
- 状態: Approve with follow-up
- レビュー詳細: [./file-ref-symlink-diagnostics.review.md](./file-ref-symlink-diagnostics.review.md)
- 日付: 2026-05-07
