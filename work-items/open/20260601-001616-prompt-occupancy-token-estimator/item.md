---
id: 20260601-001616-prompt-occupancy-token-estimator
slug: prompt-occupancy-token-estimator
title: Token estimator must keep prompt occupancy accounting whole
status: open
kind: task
priority: P1
labels: [compaction, token-accounting]
created_at: 2026-06-01T00:16:16Z
updated_at: 2026-06-01T00:41:18Z
assignee: null
legacy_ticket: null
---

## Background

New sessions can compact on the first turn even when the actual request does not exceed the configured compact thresholds. A representative session showed the first measured request at `history_len=1` with `input_total_tokens=11124`, then a mid-turn `run_completed` with `result="yielded"`, followed by a new segment with `compacted_from.at_turn_index=1`.

The suspected cause is token accounting that combines unlike properties: provider `input_total_tokens` measures the whole prompt occupancy, while current estimator paths use only history serialization bytes as the denominator. This effectively treats system/developer/tool schema/resident memory overhead as if it belonged to the history prefix, so first-turn history growth can be overestimated and trip `request_threshold`.

The fix should keep compact/request-threshold accounting focused on whole-request prompt occupancy instead of splitting system and history into a false exact model. Prune behavior is not in scope for this ticket; prune metrics may appear in the same logs but are not the cause of the first-turn compact.

## Acceptance criteria

- Compact/request-threshold estimation pairs measured `input_total_tokens` with bytes or another size measure for the same full request shape, not history-only bytes.
- Exact usage records are treated as authoritative for the measured request occupancy at their recorded request shape/prefix.
- Unmeasured request occupancy extrapolation no longer applies `total_input_tokens / history_bytes`.
- A regression test covers a fresh session / one prior usage record case where fixed prompt overhead is large and first-turn tool history growth must not trigger compact solely from the old overestimation.
- Session/log diagnostics remain sufficient to distinguish prune activity from compact/yield activity when investigating threshold behavior.
