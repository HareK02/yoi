# session-metrics

`session-metrics` records measurements in append-only Session logs and retrieves them
through dedicated read/export paths. It handles measurement types and Segment lineage;
callers choose what to measure and calculate charges. Measurements are not exposed in
ordinary conversation snapshots.

## Design entry points

- [`Metric`]: missing values and correlation with later measurements
- [`LocatedMetric`]: storage locations and compaction lineage
- [`read_session_metrics`]: read boundary for aggregation across Segments
