# メモリ機構: consolidation skip 表示と invalid staging の観測性

## 背景

`memory-audit-log` 実装後、TUI actionbar に `memory consolidation skipped: no_staging_entries` が表示されるようになった。これは正常な idle/no-op でも頻繁に発生するため、actionbar 上では「何かが skip され続けている」ように見える。

また、workspace に `.insomnia/memory/_staging/*.json` が残っているのに、現行 schema として読めないため valid staging entry が 0 件になり、`no_staging_entries` と区別できない場合がある。今回観測された主因は旧 schema の staging file で、`source.session_id` を持ち、現行 schema が要求する `source.segment_id` を持たないものだった。

旧 schema staging の後方互換・migration は求めない。この workspace に残っている旧 staging は必要に応じて手動で削除 / 退避する。実装としては、壊れた staging / 現行 schema として読めない staging がある場合に、`no_staging_entries` と誤認しない観測性だけを持たせる。

## 方針

Audit log には worker の skip / no-op を残してよいが、actionbar には人間が見る価値のある memory worker 状態だけを出す。

特に以下は actionbar に出さない。

- `no_staging_entries`
- successful consolidation の直後、backlog drain 終端確認で出る空確認 skip
- post-run / periodic check で staging が本当に空だっただけの skip
- 通常運用で頻出する no-op / idle skip

一方で、以下は actionbar に出してよい。

- consolidation の started / completed
- record changes を伴う completed
- failed / error
- invalid staging が存在するなど、人間が確認すべき状態

staging directory に file があるが current schema として読めない場合は、audit log 上で invalid staging の存在と件数が分かるようにする。actionbar に出す場合も、`no_staging_entries` ではなく invalid staging と分かる文言にする。

## 要件

- consolidation が `completed` した直後の drain 確認で発生する idle skip が、直前の actionbar 表示を上書きしない。
  - 例: `completed_record_changes` の直後に `no_staging_entries` を actionbar へ出さない。
  - audit log へ残すかどうかは実装判断でよいが、UI event としては抑制する。
- 通常の post-run check / periodic check で staging が本当に空の場合も、actionbar に `no_staging_entries` を出さない。
  - idle/no-op 状態は audit log 側で確認できればよい。
- `threshold_not_reached` は通常運用の no-op として扱い、actionbar へ常時表示しない。
  - audit log 側では `no_staging_entries` と区別して記録する。
- `list_staging_entries` あるいはその呼び出し側で、invalid / parse-failed staging file の存在を区別できるようにする。
  - 少なくとも invalid count が audit reason または structured field から分かる。
  - 例: `no_valid_staging_entries invalid=6`。
- `threshold_not_reached` と `no_valid_staging_entries` / `invalid_staging_entries` は区別される。
- old schema staging file の自動 migration / 自動削除 / 自動 archive はしない。
  - 後方互換は不要。
  - 既存 workspace の旧 staging は手動整理でよい。
  - 実装側では、現行 schema として読めない staging を invalid として観測できればよい。

## 完了条件

- successful consolidation の直後に actionbar が `no_staging_entries` で上書きされない。
- staging が本当に空の periodic/post-run check は actionbar にノイズを出さない。
- `threshold_not_reached` が actionbar を継続的に上書きしない。
- staging directory に parse 不能な `.json` がある場合、audit log が `no_staging_entries` ではなく invalid staging の存在を示す。
- invalid staging を actionbar に出す場合、`no_staging_entries` ではなく invalid staging と分かる表示になる。
- consolidation skip / invalid staging / actionbar 抑制の挙動を確認する test がある。
- `cargo fmt --check` と関連 crate の test が通る。

## 範囲外

- old schema staging file の自動 migration。
- old schema staging file の自動削除 / archive。
- actionbar transient notice 汎用 API の実装。
- memory audit log の保存形式の大幅変更。
