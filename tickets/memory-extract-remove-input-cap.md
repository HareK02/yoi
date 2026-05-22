# Memory extract: input occupancy cap を撤廃し backlog 消費を詰まらせない

## 背景

実セッションで以下の notice が繰り返し出ている。

```text
[notice] pod: memory extract failed: Aborted: extract worker input occupancy exceeded 30000 tokens
```

既存の `memory-extract-occupancy-cap` 対応では、`extract_worker_max_input_tokens` の測定方法を累積 input token から現在 prompt occupancy に直した。しかし、実運用上の問題はまだ残っている。extract は前回 pointer 以降の未処理 history slice を処理して pointer を進める機構なのに、その未処理 slice を入力にした extract worker 自身が input occupancy cap で abort すると、pointer が進まず、次回は同じ slice にさらに新規履歴が足されてより大きくなる。

つまり deterministic な cap 超過が、backlog 解消ではなく backlog 固着を生む。

現行実装では `run_extract_once` が以下を丸ごと extract input にする。

```rust
items_to_extract = worker.history()[processed_history_len..current_history_len].to_vec()
```

その後 `MemoryExtractWorkerInterceptor` が extract worker の pre-request context を `llm_worker::token_counter::total_tokens` で測り、`extract_worker_max_input_tokens` を超えると `Cancel` する。

一方で `memory::extract::build_extract_input` は raw `ToolResult.content` ではなく `ToolResult.summary` だけを render するため、巨大 tool output 本文そのものを extract に渡しているわけではない。それでも未処理 range が長い、summary / user / assistant text / tool arguments が多い、または一度失敗して pointer が止まった場合には input が cap を超えうる。

## 判断

`extract_worker_max_input_tokens` は撤廃する。extract の主目的は未処理 session slice を消費して staging に活動ログを残すことであり、入力が大きいことを deterministic abort 条件にすると目的と衝突する。

暴走防止は既存の `extract_worker_max_turns` で扱う。model context limit による API failure は通常の worker failure として扱い、input occupancy cap によって事前に pointer を止める設計はやめる。

将来、model context error が実問題になる場合は、extract source range を token / rendered text budget で chunk 分割して順次消費する設計を別途入れる。その場合も「cap 超過で abort」ではなく「収まる chunk を選んで pointer を進める」方向にする。

## 要件

- Manifest から `memory.extract_worker_max_input_tokens` を撤廃する。
  - `MemoryConfig` / cascade / docs / defaults から削除する。
  - 後方互換 shim は入れない。旧 field を書いた manifest は unknown field として明示的にエラーになること。
- `MemoryExtractWorkerInterceptor` を削除する。
  - extract worker への `UsageTracker` 配線も、input cap のためだけなら削除する。
  - `extract_worker_max_turns` は維持し、tool-loop 暴走防止として使う。
- `run_extract_once` は、現行通り pointer 以降の未処理 range を extract worker に渡し、成功時に pointer を進める。
  - cap 超過による deterministic abort で pointer が止まり続ける経路をなくす。
  - LLM/API failure や tool failure の場合は従来通り pointer を進めない。
- extract input が `ToolResult.content` ではなく `ToolResult.summary` のみを render する前提を test / comment で明確にする。
  - 大きな tool output 本文は extract input に入らないことを regression test にする。
- notice / alert 文言から `extract worker input occupancy exceeded ... tokens` が発生しないようにする。
- docs を更新する。
  - `docs/manifest.toml`
  - `docs/plan/memory.md`
  - 必要なら `crates/manifest/src/defaults.rs` / `crates/manifest/src/lib.rs` の doc comment

## 完了条件

- `memory.extract_worker_max_input_tokens` が設定項目として存在しない。
- 旧 `extract_worker_max_input_tokens` を manifest に書くと parse / validation error になる。
- 未処理 range が大きい場合でも、extract worker が input occupancy cap で abort しない。
- `ToolResult.content` に巨大文字列が入っていても、`build_extract_input` は summary のみを含める test がある。
- `extract_worker_max_turns` による暴走防止は維持される。
- `cargo fmt --check`
- `cargo check --workspace`
- `cargo test -p manifest -p memory -p pod`

## 範囲外

- extract source range の chunk 分割実装。
- oversized single item の truncation / skeleton 化。
- consolidation worker の budget 設計変更。
- memory audit log の実装。
- extract / consolidation の採択ロジック変更。

## 関連

- `tickets/memory-audit-log.md`
- 過去対応: `memory-extract-occupancy-cap`（測定方法を累積から occupancy に直したもの。今回の対象は cap の存在自体）
