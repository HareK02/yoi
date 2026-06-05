Merged and completed.

Implementation:
- Merged branch `prompt-occupancy-token-estimator` into `develop` with `merge: prompt occupancy estimator`.
- `llm-worker` token counter extrapolation now keeps exact measured prompt occupancy authoritative and no longer extrapolates one-measurement growth via `total_input_tokens / history_bytes`.
- Extrapolation past the latest measurement uses a measured incremental span rate when available; otherwise it adds a conservative byte fallback for the unmeasured delta.
- Added pod interceptor regression coverage for the fresh-session / one-measurement overestimation case.

Validation after merge:
- `cargo test -p llm-worker token_counter` passed.
- `cargo test -p pod pre_llm_request_does_not_yield_from_single_measurement_history_rate_projection` passed.
- `./tickets.sh doctor` passed.

Review:
- External reviewer approved with no blockers.
