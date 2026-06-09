Implemented, reviewed, merged, and validated.

Summary:
- Added `response_batches` metrics to session analytics JSON output.
- Added best-effort assistant-response grouping from consecutive top-level `assistant_item` JSONL entries.
- Added response-level tool metrics: total responses, tool-call responses, total tool calls, per-response stats/histogram, and top tool-call responses.
- Added Edit batching metrics: responses containing Edit, total Edit calls, calls per response, same-file multi-Edit responses, files touched per Edit response, and large-argument-compatible summaries.
- Added edit round-trip metrics for pure edit-only streaks and interrupted/annotated sequences.
- Preserved privacy boundary: no raw user input, raw tool arguments, raw file contents, raw session snippets, or raw tool output content in default output.
- Kept diagnostics as observations/correlations, not blame or policy enforcement.

Implementation:
- Coder commit: `963db29 analytics: add response batching metrics`
- Reviewer approved with no blocking findings.
- Merge commit: `c837fbc merge: add session analytics response batching`

Validation after merge:
- `cargo test -p session-analytics`
- `cargo test -p yoi run_session_analyze_outputs_json`
- `cargo fmt --check`
- `git diff --check`
- `cargo check --workspace`
- `cargo run -q -p yoi -- ticket doctor`
- `nix build .#yoi`
