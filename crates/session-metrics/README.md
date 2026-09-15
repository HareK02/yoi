# session-metrics

Session 単位の append-only な観測値を既存 session-log に記録し、明示的な
metrics 読取 / JSONL export 経路で取り出すための小さなヘルパークレートです。

- 保存先は `session-store` の `LogEntry::Extension`
- extension domain は `metrics`
- metric は `name / ts / dimensions / value / correlation_id` の最小 envelope
- `record_metric` で指定した Session / Segment に append する
- `read_segment_metrics` は 1 Segment、`read_session_metrics` は Session 内の全
  Segment を読み、各 metric に `segment_id` と `compacted_from` を付ける
- `export_metrics_jsonl` はその located metric を newline-delimited JSON にする
- 通常の Session snapshot / Worker list / Worker detail は Extension を公開しない

compaction は `compact.start` を source Segment、`compact.finish` と
`compact.post_request` を結果 Segment に記録する。同じ `correlation_id` と
`SegmentStart.compacted_from` により、Segment をまたぐ attempt と次の通常 LLM
request を結合できる。

```rust,ignore
use session_metrics::{
    Metric, export_metrics_jsonl, read_session_metrics, record_metric,
};

let metric = Metric::now("compact.start")
    .with_value(12_345.0)
    .with_dimension("trigger", "pre_run")
    .with_correlation_id("018f6f8a-9822-7b11-8b35-706f30313700");
record_metric(
    &store,
    location.session_id,
    location.segment_id,
    &metric,
)?;

let records = read_session_metrics(&store, location.session_id)?;
let jsonl = export_metrics_jsonl(&records)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```
