<!-- event: create author: tickets.sh at: 2026-05-30T05:49:27Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-05-30T05:50:04Z -->

## Plan

## Preflight

Classification: research-first / implementation-ready after sources are recorded.

The work is mostly data/catalog maintenance. It should begin with current provider documentation/model-list research and a short source note before editing the catalog. Implementation should be limited to `resources/models/builtin.toml` and directly related docs/tests unless research proves a provider definition is wrong.

Critical risks:
- Do not guess model IDs or context windows from memory.
- Do not add models that the current provider client cannot address.
- Do not churn provider definitions unless needed.
- If changing the default profile model, explain the product reason and verify compaction/effective window metadata.


---
