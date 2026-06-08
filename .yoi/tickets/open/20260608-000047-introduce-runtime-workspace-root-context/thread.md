<!-- event: create author: LocalTicketBackend at: 2026-06-08T00:00:47Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: decision author: hare at: 2026-06-08T00:05:33Z -->

## Decision

## Coordination note: workspace context should pair with explicit Pod identity

The runtime workspace context cleanup should align with `remove-profile-derived-pod-names` by making startup boundaries explicit:

- `--workspace <path>`: runtime workspace root.
- `--pod <name>`: runtime Pod identity for restore or fresh create.
- `--profile <selector>`: reusable Profile recipe.

Avoid preserving the old split where profile startup uses `--profile-pod-name` and non-profile startup uses `--pod`. That split allowed `SpawnConfig::resume_by_pod_name = false` to omit the runtime identity and let profile slug/source derive `pod.name`.

The intended end state is that every Pod startup path has an explicit runtime workspace and explicit Pod identity before profile resolution runs. Profile resolution should consume those runtime values, not invent them.

---
