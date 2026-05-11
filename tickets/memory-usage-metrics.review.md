# Review: Memory usage metrics

## 対象

- Ticket: `tickets/memory-usage-metrics.md`
- Branch: `memory-usage-metrics`
- Reviewed commit: `d581a35 feat: add memory usage event metrics`

## 確認内容

- 明示使用回数ログは workspace-local append-only JSONL として `.insomnia/memory/_usage/events.jsonl` に保存される。
- `MemoryRead` は `use` event として記録される。
- `#<slug>` Knowledge ref は解決成功時に `KnowledgeRef` source の `use` event として記録される。
- `/workflow` invocation は workflow record の `WorkflowInvoke` source の `use` event として記録される。
- `MemoryQuery` / `KnowledgeQuery` は usage を記録せず、検索 hit を使用回数に含めていない。
- resident injection は `resident_exposure` として `use_count` から分離されている。
- report は record ごとの `use_count`, `last_used_at`, `source_breakdown`, `resident_exposure_count`, `estimated_tokens_per_injection`, `estimated_total_resident_exposure_tokens` を返す。
- consolidation input は usage report を evidence として渡し、Knowledge 化や tidy protection の hard decision は実装していない。
- 旧方針の `n/Mtoken`, cumulative-token window, session 浮上率, frequency threshold 判定は入っていない。

## 指摘と対応

- 指摘: tidy hints に「Explicit-invoke metrics が未接続」という古い文言が残っており、今回の evidence report 接続と矛盾していた。
- 対応: 同 commit を amend し、usage evidence は soft context として扱う説明へ修正した。

## 検証

- `cargo test -p memory` passed
- `cargo test -p pod` passed
- `cargo fmt --check` は既存の unrelated rustfmt 差分により失敗するが、今回変更ファイルは対象 diff に含まれていない。

## 判断

Approve.

チケットの簡略化後仕様に対して、実装は過剰な frequency / session-rate 判定を持ち込まず、明示使用回数と resident exposure cost の観測に留まっている。マージしてよい。
