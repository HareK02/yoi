# Memory Phase 2: 累積入力トークン上限の撤去

## 背景

Phase 2 (memory.consolidation) の sub-Worker には、累積入力トークンが
`MEMORY_CONSOLIDATION_WORKER_MAX_INPUT_TOKENS = 80_000` を超えると
`PreRequestAction::Cancel` でアボートする circuit-breaker
(`MemoryConsolidationWorkerInterceptor`) が組み込まれている。
manifest からも `[memory] consolidation_worker_max_input_tokens` で
上書きできる。

実運用で、このアボートが notice として頻繁に出ることが分かった:

```
[notice] pod: memory Phase 2 consolidation failed:
Aborted: Phase 2 consolidation worker input exceeded 80000 tokens
```

原因は `build_consolidate_input` が staging 全文 + 既存 memory 全文を
毎ターン頭で詰めるため、ツール呼び出しを伴う複数ターンで累積入力が
80K に達するため。`worker.on_usage` の `input_tokens` を素直に加算
しているのでキャッシュヒット込みの値で上限を圧迫する。

そもそも Phase 2 は staging + 既存 memory を **全部読まないと判断
できない** 統合タスクであり、入力サイズで途中アボートしても staging が
未消費のまま残るので、次回 Phase 2 トリガで同じ理由で再度落ちる。
ユーザーが手で staging を間引かない限り永遠に notice が出続ける。

入力が context window に乗らない極端なケースは LLM 側で context
overflow になるため、自前で先回りして止める意味は薄い。runaway 防止は
別途 `Worker::max_turns` が存在するのでそちらに任せる方が筋がよい。

## ゴール

Phase 2 consolidation worker の累積入力トークン上限を撤去する。
runaway 対策は `Worker::max_turns` に寄せる。

## 要件

- `MemoryConsolidationWorkerInterceptor` を削除
- Phase 2 構築箇所 (`Pod::run_consolidate_once`) の interceptor 注入と
  `on_usage` 累積カウンタを撤去
- `MemoryConfig::consolidation_worker_max_input_tokens` を削除
- `manifest::defaults::MEMORY_CONSOLIDATION_WORKER_MAX_INPUT_TOKENS` を削除
- manifest cascade (`config.rs`) からも当該フィールドを削除
- 既存 manifest がこのキーを含んでいたとしても、`#[serde(deny_unknown_fields)]`
  系で読み込み時にエラーにならないよう確認 (現状 `deny_unknown_fields` を
  使っていなければ無視されるはずだが、要確認)
- 関連テスト・docs から `consolidation_worker_max_input_tokens` への
  参照を一掃する

## 範囲外

- Phase 1 (extract) 側の `extract_worker_max_input_tokens` の扱い
  (extract は session history の切り出しサイズに別の意味があるため別件)
- compact 側の `compact_worker_max_input_tokens` の扱い
- `Worker::max_turns` のデフォルト値見直し (現状の挙動で問題が出てから検討)
- 累積入力トークンを cache hit/miss で区別するなどのカウンタ精緻化

## 完了条件

- `cargo check` / `cargo test` が `manifest`, `memory`, `pod` で通る
- Phase 2 trigger が走った際、入力サイズに起因する Aborted notice が
  発生しないこと (整合性レビューで `MemoryConsolidationWorkerInterceptor`
  および対応する manifest field が完全に消えていることを確認)
