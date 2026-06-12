# テスト妥当性レビュー: lint-common
- 判定: 混在

## 確認範囲
- 対象 crate: `crates/lint-common`
- 読んだ主なファイル:
  - `crates/lint-common/src/lib.rs`
  - `crates/lint-common/src/slug.rs`
  - `crates/lint-common/src/frontmatter.rs`
  - `crates/lint-common/README.md`
- 軽く確認した利用側:
  - `crates/memory`
  - `crates/workflow`

## 現在のテストがよくカバーしていること
- `Slug` の基本的な受理/拒否ケースは押さえている。
  - 1文字、複数文字、数字、ハイフンを含む正常系。
  - 空文字、先頭/末尾ハイフン、大文字、underscore、空白、非 ASCII、連続ハイフンなどの異常系。
  - 最大長 64 文字と 65 文字拒否の境界。
- `Slug` の serde deserialize 経路も最低限確認されている。
  - `Slug::parse` だけでなく、frontmatter/schema 側で効く deserialize validation の基礎確認として妥当。
- `split_frontmatter` は最小限の正常系/異常系を持っている。
  - 通常の `---\n...\n---\nbody`。
  - opening delimiter 不在。
  - closing delimiter 不在。
  - 空 body。
- テストは小さい crate の public behavior に近く、過度に内部実装へ寄ってはいない。

## 不足 / 疑問のあるテスト
- `split_frontmatter` の opening delimiter 境界が弱い。コメント上は document が `---\n` で始まる前提だが、現在のテストは「`---` が独立行であること」を検証していない。
  - 例: `---foo\n---\nbody` のような入力を拒否すべきかがテストで固定されていない。
  - これは frontmatter parser の中核 invariant なので、テスト不足として重要。
- closing delimiter の EOF ケースが未確認。
  - コメントでは closing `---` at EOF を許すように読めるが、`---\nfoo: 1\n---` のようなケースがテストされていない。
- CRLF の扱いが曖昧なままテストされていない。
  - closing delimiter 側は `\r` を落としているが、opening delimiter 側は `---\r\n` を明示的に扱っていない。
  - Windows 改行をサポート対象にするなら不足。サポート外なら拒否テストがあるとよい。
- `Slug` の仕様表示とテストが少し噛み合っていない可能性がある。
  - `lib.rs` の error message と `slug.rs` のコメントにある regex `^[a-z0-9](?:[a-z0-9-]{0,62}[a-z0-9])?$` は `foo--bar` を許す形に見える。
  - 一方、実装とテストは `foo--bar` を拒否している。
  - 連続ハイフン禁止が正なら、テスト自体は妥当だが、テストが依拠する authoritative spec がコメント/エラー文とずれている。
- `Slug` の public API 周辺の確認がやや薄い。
  - `Display`, `AsRef<str>`, `FromStr`, `into_string`, serde serialize は未テスト。
  - 重大ではないが、この crate が共通 primitive を提供する責務なら薄め。
- property/fuzz 的なテストはない。
  - 現状の実装サイズでは必須ではないが、slug validator は文字種/長さ/ハイフン位置の組み合わせが多いため、簡単な table-driven 境界テストを増やす価値はある。

## 追加候補
- `split_frontmatter` の delimiter 厳密性テストを追加する。
  - `---foo\n---\nbody` は拒否。
  - `------\nbody` は拒否。
  - opening delimiter はファイル先頭の独立行でなければならない、という仕様を固定する。
- EOF closing delimiter の正常系を追加する。
  - `---\nfoo: 1\n---` が成功し、body が空になること。
- CRLF の方針をテストで固定する。
  - サポートするなら `---\r\nfoo: 1\r\n---\r\nbody`。
  - サポートしないなら明示的に error になること。
- slug の仕様とテストを同期させる。
  - 連続 `--` 禁止が正なら、error message / regex コメントもそれに合わせる。
  - そのうえで `a--b`, `a--`, `--a` など、連続ハイフンと端点ハイフンの境界を分けた test case を置く。
- `Slug` の変換 API の薄い smoke test を追加する。
  - `FromStr`
  - `Display`
  - `AsRef<str>`
  - `into_string`
  - serde serialize roundtrip
- 可能なら `is_valid_slug` と `Slug::parse` の結果が常に一致する table test を、正常/異常すべてのケースで明示する。
  - 現在も一部では確認しているが、全 case を共通 helper にすると漏れが減る。

## 実行したコマンド
- `cargo test -p lint-common`
  - 結果: pass
  - 8 unit tests passed.
  - 0 doc tests.

