<!-- event: create author: tickets.sh at: 2026-05-31T04:32:39Z -->

## Created

Created by tickets.sh create.

---

<!-- event: plan author: hare at: 2026-05-31T04:33:28Z -->

## Plan

Implementation plan:

1. Move current `crates/pod/src/main.rs` startup logic behind a `pod` crate library entrypoint, leaving the existing binary as a thin temporary wrapper.
2. Add a reserved `pod` subcommand to the `insomnia` binary that delegates to the same Pod runtime entrypoint.
3. Keep internal spawn defaults and Nix installed commands unchanged in this first step unless the change is smaller than expected; the long-term plan is still to remove `insomnia-pod` after internal callers use `insomnia pod`.
4. Add/update parser tests so `insomnia pod` is reserved and `insomnia --pod pod` remains available for a Pod literally named `pod`.
5. Validate focused CLI/parser/runtime help behavior and affected crates.


---
