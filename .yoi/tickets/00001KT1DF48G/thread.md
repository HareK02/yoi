<!-- event: create author: tickets.sh at: 2026-06-01T11:00:26Z -->

## Created

Created by tickets.sh create.

---

<!-- event: implementation_report author: hare at: 2026-06-01T11:09:35Z -->

## Implementation report

# Implementation report

## Summary

Standardized crate README files as thin responsibility-boundary documents.

## Changes

- Rewrote existing stale README files to remove public-type inventories and outdated architecture claims.
- Added concise README files for important crates that lacked them:
  - `client`
  - `lint-common`
  - `memory`
  - `pod-registry`
  - `pod-store`
  - `secrets`
  - `session-metrics`
  - `tools`
  - `workflow`
  - `yoi`
- Each crate README now follows the same shape:
  - Role
  - Boundaries
  - Design notes
  - See also
- README files link to maintained `docs/design/` or `docs/development/` docs instead of duplicating long design explanations.

## Validation

- All crates under `crates/*` have a README.
- `rg` sweep for stale public-type headings, old crate title, accidental edit text, and old product names in crate README files => no output.
- `./tickets.sh doctor` => `doctor: ok`
- `git diff --check` => passed


---

<!-- event: close author: hare at: 2026-06-01T13:22:51Z status: closed -->

## Closed

Completed crate README boundary cleanup: all crates now have concise role/boundary README files linked to maintained design/development docs; validation passed.


---
