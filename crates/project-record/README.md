# project-record

Provides timestamp encoding, validation, and collision-aware candidate allocation for record IDs.
The caller owns record persistence and concurrency control for writes.

## Main entry points

- [`encode_unix_epoch_millis`], [`decode_unix_epoch_millis`]: fixed-width ID conversion
- [`allocate_record_id`]: candidate selection avoiding existing IDs
