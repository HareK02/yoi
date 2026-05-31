<!-- event: create author: tickets.sh at: 2026-05-31T08:26:46Z -->

## Created

Created by tickets.sh create.

---

<!-- event: close author: hare at: 2026-05-31T08:29:40Z status: closed -->

## Closed

Documented the environment-variable policy and current supported surface.

Implementation:
- Added `docs/environment.md`.
- Stated the preferred design rule: avoid new environment variables when profile/manifest/config/CLI/typed secret references are better.
- Documented core path variables, runtime/socket variables, the Pod runtime command override, credential env references, external compatibility variables, and build/test/example-only variables.
- Clarified that normal runtime does not implicitly load `.env` files.
- Added cleanup direction for shared test env guards, keeping path resolution centralized, and moving toward typed secret references.
- Linked the new page from `docs/nix.md` and installed it in the Nix package docs.

Validation:
- `./tickets.sh doctor`
- `git diff --check`


---
