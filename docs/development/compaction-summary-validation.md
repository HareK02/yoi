# Compaction summary input validation (T-738)

## Baseline and scope

Implementation starts at `6a12b6cc346402b7a10208685038967171b09cb3`.
Compared with the Ticket's inspected `3263d501`, the compaction input, capture and
prompt contracts are unchanged; intervening Worker edits concern state-contract
naming, resource/manage-worker projections, and shared state. The defects reported
against `3263d501` therefore still apply to this checkout.

The change fixes mandatory previous-summary transport and the continuity policy.
It does not add Journal integration (T-745), a separate directive ledger, exploration
of historical Segments, or a comprehensive internal-compactor audit log. See
[the current design](../design/compaction.md) for the input and retention contracts.

## Acceptance evidence

| Ticket condition | Deterministic evidence |
| --- | --- |
| 1: full initial input, long summary and recent activity | `full_initial_summary_survives_small_overview_budgets_and_both_retention_extremes` observes the actual first compactor `LlmClient::Request`; full body equality, one occurrence, and no clipped duplicate in overview. Summary exceeds 4,000 characters with directives/end sentinel after the long prefix, plus 80 recent messages. |
| 2: small target/warning/fallback, context accounting, hard failure | Same matrix uses target=1 and warning/deadline settings 0/1. `inherited_summary_counts_against_hard_input_limit_and_failure_does_not_activate` compares the same overview with/without a mandatory summary above the cap: no compactor request, no replacement Segment, unchanged active metadata/history on rejection. `budget_warning_is_durable_and_keeps_full_summary_in_first_model_request` verifies the warning survives in the next request. Interceptor tests cover request-shape estimation, measured occupancy without double counting, and a warning that cannot fit. |
| 3: absent summary, restore, retained boundary, other derived context | Matrix covers no retained tail and retaining all ordinary items. `pasted_markers_and_other_derived_context_remain_ordinary_overview_material` covers absent summary, auto-read, TaskStore, ordinary system, user paste and later derived markers. `restored_legacy_summary_requires_compacted_lineage_first_position_origin_role_and_marker` covers the exact adopted old Host seed boundary and its counterexamples. Invalid or duplicate typed Host summaries fail closed. |
| 4: privacy and lightweight overview | Existing tool-argument/tool-result/reasoning omission tests remain. `host_summary_remains_private_to_compaction_not_observation_or_memory_evidence` verifies overview, search, read, and evidence exclude Host summary, other derived/system and reasoning bodies. The persisted retained-history paging fixture includes the new Host summary origin and still excludes derived content. Public provenance maps to the existing excluded `DerivedSummary` class; no public observation/Memory capability is added. |
| 5: prompt and shortening policy | Resource policy test covers initial summary, segment user messages, prohibitions, completion conditions, changes/retractions, uncertainty, TaskStore distinction and retained tail. Shrink-policy and budget-warning tests cover the corresponding priorities. Output-hard-max integration test exercises the actual rewrite path and explicit failure if output remains oversized. |
| 6: at least three chained compacts | `three_chained_compactions_transport_each_complete_predecessor_without_retaining_old_summaries` starts with a long summary carrying a late original-goal sentinel; each of three compacts receives a full predecessor and a new user directive containing temporary recovery/SubWorker collection instructions. It checks each actual initial request, one current typed summary, predecessor lineage and persisted result. |
| 7: tests/check/format and current design | Commands below pass; design input, privacy, retention, limit and assessment descriptions are updated. |

## Executed validation

During implementation, the smallest filters were run first:

- `cargo test -p worker --lib build_summary_prompt_tests::overview`
- `cargo test -p worker --lib compact_worker_`
- `cargo test -p worker --lib session_history::`
- `cargo test -p worker --lib session_capture::`
- `cargo test -p worker --lib build_summary_prompt_tests::compact_prompt_and_shrink`
- `cargo test -p worker --lib malformed_or_duplicate_host`
- `cargo test -p worker --test compact_events_test inherited_summary_counts_against_hard_input_limit_and_failure_does_not_activate`
- `cargo test -p worker --test compact_events_test` (25 tests)
- `cargo test -p session-store --lib retained_history_pages_twelve_turns`

Final default-configuration validation:

- `cargo check` at workspace root (normal compile closure, including TUI/Server)
- `cargo test -p worker` (library, integration targets, doctests)
- `cargo test -p session-store` (library, integration targets, doctests)
- `cargo fmt --all -- --check`
- `git diff --check HEAD`
- `env -u LD_LIBRARY_PATH deno run --allow-read --allow-run=git tools/check-state-contract-names.ts` (0 unapproved, 0 stale exceptions)

No added contract depends on an optional Worker feature. Full-workspace tests,
`--all-targets`, live provider calls, E2E, Nix and Docker builds were not run.

## Unverified semantic and operational behavior

Predetermined mock `write_summary` results prove transport, input policy and
replacement behavior, **not** a real model's semantic retention or comprehension.
Live semantic evaluation remains pending: use a real model across at least three
compacts, score original scope/prohibitions/completion/exception-reporting duties
and explicit subsequent changes separately from transport success, and verify that
temporary recovery procedures do not displace the continuing request. Do not credit
an echo fixture as semantic evidence.

The restore fixture exercises public `rewind_to` and the shared typed restoration
path over persisted annotated records. It is not a cold Runtime restart test.
Manual and automatic compaction use the same `compact_impl`; existing automatic
resume tests pass, but no separate live automatic-compaction semantic evaluation
was run. Initial context accounting is an estimate, not exact provider tokenization;
provider failure must still fail the run without activation.
