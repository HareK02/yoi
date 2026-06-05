# Implementation report: feature-api-authority-separation

## Worktree / branch

- Worktree: `/home/hare/Projects/yoi/.worktree/feature-api-authority-separation`
- Branch: `work/feature-api-authority-separation`

## Commit

- `4fc361f refactor: name feature host authorities explicitly`

## Summary

Clarified the `pod::feature` authority boundary by renaming the generic authority API surface to explicit host-authority terminology. This keeps feature contribution declarations separate from host-mediated capability grants and prepares the API for later Ticket built-in tools without framing internal built-ins as external plugin package grants.

## Exact renames

- `AuthorityRequest` -> `HostAuthorityRequest`
- `AuthorityGrantSet` -> `HostAuthorityGrantSet`
- `AuthorityDenial` -> `HostAuthorityDenial`
- `FeatureDescriptor::requested_authorities` -> `requested_host_authorities`
- `FeatureDescriptor::with_authority` -> `with_host_authority`
- `ToolContribution::required_authorities` -> `required_host_authorities`
- `ToolContribution::with_required_authorities` -> `with_required_host_authorities`
- `FeatureInstallReport::granted_authorities` -> `host_authority_grants`
- `FeatureInstallContext::grants()` -> `host_authority_grants()`
- `FeatureInstallError::AuthorityDenied` -> `HostAuthorityDenied`
- Internal helpers/diagnostics now use host-authority terminology where applicable.

## Changed files

- `crates/pod/src/feature.rs`

## Behavior

Preserved:

- descriptor-first validation;
- duplicate tool rejection;
- undeclared contribution rejection;
- missing required host authority install failure;
- built-in Task feature behavior;
- contribution-only built-in feature installation without host authority grants.

Added/updated tests and comments to make explicit that contributing a tool/hook/background/service descriptor is not itself a host authority grant, while per-tool host authority requirements still require a corresponding granted requested host authority.

## Validation

Coder-reported validation passed:

- `cargo test -p pod feature --lib`
- `cargo test -p pod task --lib`
- `cargo test -p pod --lib`
- `cargo test -p llm-worker --lib`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `git diff --check`
- `./tickets.sh doctor`
- `nix build .#yoi --no-link`

Reviewer-rerun validation passed:

- `git diff --check develop...HEAD`
- `cargo test -p pod feature --lib`

## Review status

External sibling reviewer approved with no blockers and no required non-blockers before merge.

## Unresolved risks / follow-ups

The existing `HostAuthorityGrantSet::grant_all(&descriptor.requested_host_authorities)` behavior remains a builtin-only scaffold, not a real external plugin approval resolver. This is unchanged and explicitly outside this ticket's scope.

## Ready for merge

Yes. This clears the API naming prerequisite for `ticket-built-in-feature-tools`.
