<!-- event: create author: hare at: 2026-05-29T22:28:50Z -->

Created as a follow-up to the closed manifest profiles work item after reviewing the original intent and the current built-in Nix profile shape.

---

<!-- event: plan author: planning-pod at: 2026-05-29T22:36:45Z -->

Implementation plan written to `artifacts/implementation-plan.md`. Key recommendation: introduce a typed semantic profile artifact and manifestization step, move/centralize model catalog context-window resolution so compaction can derive from model metadata, and resolve builtin profiles in-process so normal default startup does not require external `nix`.
