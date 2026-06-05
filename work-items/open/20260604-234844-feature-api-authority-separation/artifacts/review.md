# External review: feature-api-authority-separation

## 1. Result

approve

## 2. Summary of implementation

The implementation is a focused `pod::feature` API cleanup in `crates/pod/src/feature.rs` only. It renames the generic authority API surface to explicit host-authority naming:

- `AuthorityRequest` -> `HostAuthorityRequest`
- `AuthorityGrantSet` -> `HostAuthorityGrantSet`
- `AuthorityDenial` -> `HostAuthorityDenial`
- `FeatureDescriptor::requested_authorities` -> `requested_host_authorities`
- `FeatureDescriptor::with_authority` -> `with_host_authority`
- `ToolContribution::required_authorities` / builder -> `required_host_authorities` / `with_required_host_authorities`
- `FeatureInstallReport::granted_authorities` -> `host_authority_grants`
- `FeatureInstallContext::grants()` -> `host_authority_grants()`
- `FeatureInstallError::AuthorityDenied` -> `HostAuthorityDenied`

The code comments and tests now describe contribution declarations as descriptor-approved contributions, not host authority grants. A new focused test covers a tool that needs a host authority: contribution declaration alone is not enough, while an explicit requested host authority grants the install-time host-authority requirement under the current grant-all scaffold.

## 3. Requirement-by-requirement assessment

- Generic authority ambiguity removed or clarified: satisfied. The changed source no longer exposes the generic `AuthorityRequest`, `AuthorityGrantSet`, `AuthorityDenial`, `requested_authorities`, `required_authorities`, `granted_authorities`, or `grants()` names in the live Rust API; all are explicitly host-authority named.
- Contribution declarations remain separate from host authority grants: satisfied. Tool/hook/background/service declarations remain descriptor contribution data, while host authorities are carried through `requested_host_authorities` and `host_authority_grants`.
- Built-in contribution registration does not require host authority merely to contribute descriptors: satisfied. `ToolContribution::new` defaults to no `required_host_authorities`, background tasks/services remain descriptor/report contributions, and Task installs with empty host-authority grants.
- Missing required host authority still fails feature installation where appropriate: satisfied for the current install surface. `ToolContribution::with_required_host_authorities` still rejects install when the corresponding host authority is absent from the grant set, and the new test covers this path.
- Descriptor-first validation and duplicate tool rejection preserved: satisfied. The existing undeclared contribution, tool-name mismatch, duplicate tool, background/service declaration, and worker materialization tests remain in place and passed in focused validation.
- Built-in Task feature behavior unchanged: satisfied by diff scope and focused tests. Task descriptor/install behavior still reports no host authorities and installs the same Task tool/hook set.
- No unrelated scope expansion: satisfied. The diff is limited to `crates/pod/src/feature.rs`; it does not introduce Ticket tools, Ticket backend authority grants, plugin loading, MCP, WASM/sandbox runtime, approval protocol, crate extraction, Hook behavior changes, Task behavior changes, or broad refactors.
- Tests/docs/comments reflect the distinction: satisfied. Public comments and test names/assertions now consistently use host-authority terminology for this API surface, and tests explicitly cover contribution-only built-ins and host-authority-gated tools.
- Validation sufficient: satisfied for this review. I reran focused validation and inspected the diff/source for the listed requirements.

## 4. Blockers

None.

## 5. Non-blockers / follow-ups

None required before merge.

Notes for later external-plugin work:

- `HostAuthorityGrantSet::grant_all(&descriptor.requested_host_authorities)` remains the existing builtin-only scaffold, not a real host policy/user approval resolver. This is unchanged and within this ticket's non-goals, but it must be replaced before enabling untrusted external plugins.
- `HostAuthorityRequest::required` versus `optional` remains future-facing under the current grant-all scaffold. No behavior change is introduced here.

## 6. Validation assessed or rerun

Rerun from `/home/hare/Projects/yoi/.worktree/feature-api-authority-separation`:

- `git diff --check develop...HEAD` — passed.
- `cargo test -p pod feature --lib` — passed: 33 passed, 0 failed, 252 filtered out.

Additional inspection:

- Reviewed ticket and delegation intent.
- Reviewed `git diff develop...HEAD`.
- Confirmed the implementation diff touches only `crates/pod/src/feature.rs`.
- Searched Rust sources for the old generic authority names and found no live-code API leftovers, only new host-authority names or historical work-item text outside source.
- Checked worktree status after validation; no tracked changes.

Not rerun:

- Full workspace checks, broader package tests, `./tickets.sh doctor`, and `nix build .#yoi --no-link` were not rerun for this focused external review.

## 7. Residual risk

Residual risk is low for this ticket. The change is mostly API naming plus tests, and focused validation passed. The main remaining risk is pre-existing: the feature registry still automatically grants requested host authorities, so it is not yet a real external-plugin authority enforcement layer. That risk is explicitly outside this ticket's implementation scope and does not block this host-authority naming clarification.
