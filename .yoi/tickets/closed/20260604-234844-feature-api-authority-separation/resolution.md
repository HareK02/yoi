Feature API authority separation is complete and merged.

Implementation:

- `4fc361f refactor: name feature host authorities explicitly`
- merge commit: `b46ea65 merge: clarify feature host authorities`

Summary:

- Renamed the generic feature authority API surface to explicit host-authority terminology:
  - `AuthorityRequest` -> `HostAuthorityRequest`
  - `AuthorityGrantSet` -> `HostAuthorityGrantSet`
  - `AuthorityDenial` -> `HostAuthorityDenial`
  - `requested_authorities` -> `requested_host_authorities`
  - `required_authorities` -> `required_host_authorities`
  - `granted_authorities` -> `host_authority_grants`
  - `grants()` -> `host_authority_grants()`
  - `FeatureInstallError::AuthorityDenied` -> `HostAuthorityDenied`
- Preserved descriptor-first validation, duplicate tool rejection, undeclared contribution rejection, missing host-authority install failure, and built-in Task feature behavior.
- Added/updated tests/comments to make contribution declarations separate from host authority grants.
- Did not implement Ticket tools, external plugin loading, approval/resume protocol, MCP, WASM/sandbox runtime, feature crate extraction, Hook behavior changes, or Task behavior changes.

Review:

- External sibling reviewer approved with no blockers and no required non-blockers.
- Residual note: `HostAuthorityGrantSet::grant_all(&descriptor.requested_host_authorities)` remains the existing builtin-only scaffold, not a real external-plugin approval resolver. This is unchanged and remains future work.

Post-merge validation passed:

- `cargo test -p pod feature --lib`
- `cargo test -p pod task --lib`
- `cargo test -p pod --lib`
- `cargo test -p llm-worker --lib`
- `cargo fmt --check`
- `git diff --check`
- `./tickets.sh doctor`
- `cargo check --workspace --all-targets`
- `nix build .#yoi --no-link`

This clears the API naming prerequisite for `ticket-built-in-feature-tools`.
