# テスト妥当性レビュー: memory
- 判定: 概ね良い

## 確認範囲
- 対象 crate: `crates/memory` / パッケージ `memory`
- 読んだ主な範囲:
  - `crates/memory/README.md`
  - `src/lib.rs`
  - `src/workspace.rs`
  - `src/linter/*`
  - `src/schema/*`
  - `src/tool/{read,write,edit,delete,query}.rs`
  - `src/extract/*`
  - `src/consolidate/*`
  - `src/resident.rs`
  - `src/usage.rs`
  - `src/audit.rs`
- 変更は行っていない。

## 現在のテストがよくカバーしていること
- crate の責務に沿った主要な pure / filesystem-local invariants はかなり広く押さえられている。
  - `.yoi/memory` / `.yoi/knowledge` の path classification、opaque subtree (`_staging`, `_usage`, `_logs`) の除外、invalid slug / nested path reject。
  - workspace root resolution で `.yoi` project records だけを memory marker と見なさない挙動。
  - linter の frontmatter 必須 field、`replaced_by` の存在確認・self reference、body size、Knowledge `model_invokation` description cap、same slug create reject、similar slug warning。
  - `MemoryRead` / `MemoryWrite` / `MemoryEdit` / `MemoryDelete` / `MemoryQuery` / `KnowledgeQuery` の基本成功・基本失敗・slug rule・workflow kind 非公開。
  - query の list/search、case-insensitive search、excerpt context、result limit、Knowledge kind filter、frontmatter search、query が usage event を増やさないこと。
  - resident summary / resident knowledge collection の missing/malformed/empty/並び順/`model_invokation` filter。
  - extract staging, extract pointer fold, extract input rendering で tool result content や reasoning を落とすこと。
  - consolidation staging list, invalid staging count, lock acquire/release/stale handling, tidy hints, consolidate prompt sections。
  - usage event aggregation で explicit use と resident exposure を分けること。
  - audit JSONL append、record snapshot diff count、hash format。
- テスト数も十分あり、`cargo test -p memory` は 108 tests + doc-tests 0 で成功している。
- 大半が temporary directory を使う unit/integration-light test なので、crate 単体の filesystem behavior を検証する形として妥当。

## 不足 / 疑問のあるテスト
- full flow の結合テストは薄い。
  - extract payload → staging → consolidation input → memory tools write/edit/delete → audit/usage までの一連の流れは、個別部品ごとにはあるが、run-level の不変条件としては検証されていない。
  - Pod / Worker 経由の real tool registry や permission scope との接続はこの crate 単体ではほぼ未検証。
- linter の網羅性に穴がある。
  - `InvalidStatus`、unknown/extra frontmatter fields、malformed date、`sources` / `last_sources` の shape、request / knowledge / summary の必須 field failure が体系的には確認されていない。
  - `replaced_by` cycle detection の実シナリオは弱い。`references.rs` の test は unknown reference で 1 error になる smoke に近く、既存 A→B / B→A のような cycle report を明確には固定していない。
  - `LowImportanceLargeRecord` と `SourcesOverflow` warning は tidy 側では一部見ているが、linter warning と tool output/audit への伝播としては薄い。
- tool の edge case が不足している。
  - `MemoryRead` の `offset` / `limit` / truncation summary、limit=0 の `.max(1)` 挙動、空ファイル・末尾改行なしの line numbering が未検証。
  - `MemoryEdit` の `replace_all`, duplicate old_string reject, old_string empty, identical replacement, non-UTF-8 file, summary/knowledge edit path が未検証。
  - `MemoryWrite` の invalid slug、summary with slug、Knowledge/Request 作成、write success audit contents、warning summary/audit reason が未検証。
  - `MemoryDelete` は成功 path のみで、missing file、summary slug forbidden、invalid slug、workflow kind reject、audit failure record が未検証。
- 誤解を招く / 弱い test がある。
  - `write_aggregates_multiple_errors` は名前に反して、実際の assertion は `status` / `missing` の substring だけで、body too long など複数 error aggregation を確認していない。現実の linter は frontmatter parse failure で早期 return するため、この test は「複数 error が集約される」保証になっていない。
  - 一部の assertion は `summary.contains("Created")` や error message substring など、契約の中核ではなく表示文言寄り。大きな脆さではないが、重要な audit/hash/file-content invariant を直接見る方が強い箇所がある。
- filesystem failure / concurrency はほぼ未検証。
  - write/edit/delete の permission error、directory/file collision、partial write、外部同時変更は現状未カバー。
  - consolidation lock は live pid / stale pid / cleanup は見ているが、corrupt lock overwrite や `release_only` は明示テストがない。
- query / resident の malformed handling はある程度あるが、KnowledgeQuery の kind filter 時に malformed frontmatter を skip する仕様、malformed でも query だけなら body hit できる仕様は直接の regression test があるとよい。
- schema strictness が仕様なら危険。
  - `frontmatter::deserialize_strict` という名前だが、schema structs 側に `deny_unknown_fields` が見当たらず、unknown field reject の test もない。extra field を許す設計なら問題ないが、「strict」を期待するならテスト・実装とも不足。

## 追加を提案するテスト
- 高優先度:
  - `write_aggregates_multiple_errors` を実際に複数 lint error を確認する test に直すか、名前を現実に合わせる。
  - `replaced_by` cycle の具体例を追加する: existing `a -> b`, `b -> c` に対して `c -> a`、または existing cycle を含む場合の `ReplacedByCycle`。
  - `MemoryEdit` の `replace_all=false` duplicate reject / `replace_all=true` multi replace / rollback after lint failure / audit failure record を追加。
  - `MemoryRead` の `offset` / `limit` / truncation / limit=0 を追加。
  - `MemoryDelete` の missing file・summary slug forbidden・invalid slug・workflow kind reject を追加。
- 中優先度:
  - linter の invalid status、malformed timestamps、request/knowledge/summary の必須 field、Knowledge `last_sources` malformed、warning propagation を追加。
  - KnowledgeQuery の malformed frontmatter behavior: kind filter では skip、query-only では body match 可能、という仕様を固定。
  - write/edit/delete/read の audit log JSON を success/failure それぞれで軽く確認する。
  - consolidation lock の corrupt lock overwrite と `release_only` の staging preservation を追加。
- 低〜中優先度:
  - extract/consolidate の miniature end-to-end test を 1 本追加する。実 Worker までは不要でも、`write_staging` → `list_staging_entries` → `build_consolidate_input` → tool write/edit の組み合わせで主要データ形状を固定できる。
  - unknown frontmatter fields を許す/拒否する方針を決め、方針に応じた test を追加する。
  - non-UTF-8 / directory collision / permission failure は OS 依存を避けつつ、可能な範囲で error path regression を足す。

## 実行したコマンド
```text
cargo test -p memory
```
結果: 成功。108 passed, 0 failed, doc-tests 0。

```text
cargo test -p memory -- --list
```
結果: 成功。108 tests listed, doc-tests 0。

