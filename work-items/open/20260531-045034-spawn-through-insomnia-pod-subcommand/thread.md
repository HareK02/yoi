<!-- event: create author: tickets.sh at: 2026-05-31T04:50:34Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-05-31T04:51:14Z -->

## Plan

Implementation plan:

1. Find all internal `insomnia-pod` executable resolution paths (`client::spawn_pod`, `SpawnPod`, `RestorePod`/discovery, tests/dev helpers).
2. Introduce a typed runtime command structure with `program` and `prefix_args`, avoiding shell-string parsing.
3. Make the default runtime command for callers running from `insomnia` use `current_exe()` + `pod` prefix args.
4. Preserve explicit override behavior for development/debugging; if the existing env var cannot safely support prefix args, keep it as executable-only and document that it bypasses the unified default.
5. Update tests to assert prefix args are included and existing spawn/restore behavior is unchanged.
6. Leave Nix/package removal of `insomnia-pod` to a follow-up unless this change proves complete and low-risk.


---
