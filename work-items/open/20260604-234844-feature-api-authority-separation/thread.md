<!-- event: create author: tickets.sh at: 2026-06-04T23:48:44Z -->

## Created

Created by tickets.sh create.

---

<!-- event: decision author: hare at: 2026-06-04T23:50:15Z -->

## Decision

# Decision: separate internal feature modules from external-plugin authority

Internal modules extracted from Pod implementation files should not be treated as if they require the external-plugin permission model.

For an internal built-in module such as Task tools:

- the feature registry is an API/registration boundary;
- descriptor-declared contributions are reconciled at install time;
- normal ToolRegistry and PreToolCall permission behavior remains authoritative;
- host state such as `TaskStore` can be passed by the Pod host constructor;
- requested host authorities should normally be empty.

The external-plugin authority model remains necessary for sandbox/object-capability grants when plugin code receives dangerous host APIs such as filesystem, network, secrets, model-visible durable notification/history append, Pod-management façade, persistent state, or authority-bearing service access.

This split should be implemented separately from the Task tools extraction. The Task tools extraction should validate the contribution-only built-in module path without solving external plugin approval.


---

<!-- event: decision author: hare at: 2026-06-05T00:49:53Z -->

## Decision

# Decision: authority handles live in Hook contexts, not Hook return effects

The internal-module and external-plugin authority split should treat host-authority APIs as handles supplied by the host, including inside Hooks.

Implications:

- Hook return values remain per-hook-point flow-control actions.
- Side effects such as durable model-visible SystemItem append are performed through typed host handles on event-specific Hook contexts.
- Built-in internal modules may receive handles according to host policy without user-facing external-plugin approval.
- Future external plugins receive only the handles allowed by their approved host authorities.
- The main API should not be “return an effect and let the host reject it at runtime.” Rejection remains defense-in-depth for malformed calls, missing handles, bounds, and policy violations.
- Do not model every authority combination as a distinct Hook context type. Use event-specific context types with authority-specific handles whose constructors are host-owned.

This preserves the clean distinction: contribution declarations are descriptor-locked; dangerous host APIs are represented by host-created handles; normal tool permission remains the per-call execution gate.


---

<!-- event: plan author: hare at: 2026-06-05T04:54:33Z -->

## Plan

Preflight result: `implementation-ready`.

`ticket-built-in-feature-tools` should not be implemented until this boundary is clarified, because Ticket tools need to be internal built-in feature contributions while Ticket backend operations remain typed host authority, not arbitrary filesystem scope or external plugin package approval.

Implementation intent:

- Clarify `pod::feature` naming around host authority grants.
- Keep contribution declarations and descriptor reconciliation separate from host authority requests/grants.
- Preserve built-in Task feature behavior as the current contribution-only example.
- Avoid external plugin loading, real approval protocol, Ticket tool implementation, Hook behavior changes, or broader crate moves.

Detailed delegation intent is in `artifacts/delegation-intent.md`.

Note: main workspace currently has an unrelated dirty `README.md` change. This feature work must not touch or commit that file.


---

<!-- event: review author: hare at: 2026-06-05T05:09:36Z status: approve -->

## Review: approve

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


---

<!-- event: implementation_report author: hare at: 2026-06-05T05:09:37Z -->

## Implementation report

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


---
