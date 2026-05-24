# メモリ機構: consolidation skip 表示と invalid staging の観測性

## 背景

`memory-audit-log` 実装後、TUI actionbar に `memory consolidation skipped: no_staging_entries` が表示されるようになった。これは一見すると「staging が無い」状態に見えるが、実際の workspace には `.insomnia/memory/_staging/*.json` が残っている場合がある。

現状の原因は二つある。

1. `run_consolidate_once` が成功後に backlog drain のため loop し、直後の空確認で `skipped: no_staging_entries` を emit する。これにより、直前の `completed_record_changes` 表示が actionbar 上で上書きされる。
2. 旧 schema の staging file は `source.session_id` を持ち、現行 schema が要求する `source.segment_id` を持たないため parse で skip される。結果として、ファイルは存在するのに valid entry が 0 件となり、`no_staging_entries` と記録される。

この状態は機能停止ではないが、観測面として misleading であり、memory 機構の稼働状況を人間が誤解しやすい。

## 方針

Audit log には worker の skip / no-op を残しつつ、actionbar には人間が見る価値のある状態だけを出す。特に successful consolidation の直後に発生する drain 終端確認の `no_staging_entries` は actionbar に出さない。

また、staging directory にファイルがあるが current schema として読めない場合は、`no_staging_entries` ではなく invalid staging が存在することを audit log から分かるようにする。

## 要件

- consolidation が `completed` した直後の drain 確認で発生する idle skip が、直前の actionbar 表示を上書きしない。
  - 例: `completed_record_changes` の直後に `no_staging_entries` を actionbar へ出さない。
  - audit log へ残すかどうかは実装判断でよいが、UI event としては抑制する。
- 通常の post-run check で staging が本当に空の場合も、actionbar に `no_staging_entries` を毎回出さない。
  - idle/no-op 状態は audit log 側で確認できればよい。
- `list_staging_entries` あるいはその呼び出し側で、invalid / parse-failed staging file の存在を区別できるようにする。
  - 少なくとも invalid count が audit reason または structured field から分かる。
  - 例: `no_valid_staging_entries invalid=6`。
- `threshold_not_reached` と `no_valid_staging_entries` / `invalid_staging_entries` は区別される。
- 既存の old schema staging を自動 migration / delete しない。
  - `source.session_id` と `source.segment_id` は意味が違う可能性があるため、この ticket では観測性改善に留める。
  - 必要なら後続で archive/drop/migration 方針を決める。

## 完了条件

- successful consolidation の直後に actionbar が `no_staging_entries` で上書きされない。
- staging directory に parse 不能な `.json` がある場合、audit log が `no_staging_entries` ではなく invalid staging の存在を示す。
- staging が本当に空の periodic/post-run check は actionbar にノイズを出さない。
- consolidation skip / invalid staging の挙動を確認する test がある。
- `cargo fmt --check` と関連 crate の test が通る。

## 範囲外

- old schema staging file の自動 migration。
- old schema staging file の自動削除 / archive。
- actionbar transient notice 汎用 API の実装。
- memory audit log の保存形式の大幅変更。
