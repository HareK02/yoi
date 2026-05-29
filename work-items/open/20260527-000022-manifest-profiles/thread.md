<!-- event: migration author: tickets.sh-migration at: 2026-05-27T00:00:22Z -->

## Migrated

Migrated from TODO.md entry without a legacy ticket file. No legacy review file was present at migration time.

---

<!-- event: plan author: hare at: 2026-05-29T16:09:27Z -->

## Plan

Implementation will proceed through a child orchestrator Pod in a dedicated worktree as an experiment in nested Pod delegation.

Initial implementation target:

- Introduce Nix profile resolution as a new manifest source before the existing manifest cascade.
- Start with explicit path-based profiles; discovered-name/default selection and rich TUI picker can be staged after the core resolver if necessary.
- Provide a minimal bundled Nix helper that can produce a typed resolved manifest/config artifact.
- Keep existing TOML manifest loading as compatibility/debug/test infrastructure.
- Persist enough profile identity and resolved snapshot data for future restore semantics; do not silently re-evaluate profiles on resume.
- Secret values must remain references only; plaintext secrets are out of scope for the profile resolver.

The child orchestrator may split implementation among sub-Pods, but final merge/close remains parent-side.


---
