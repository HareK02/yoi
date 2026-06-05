# Delegation intent: feature API authority separation

## Classification

`implementation-ready` as a bounded API cleanup/refinement.

This ticket is a prerequisite for `ticket-built-in-feature-tools` because Ticket tools must be installed as internal built-in feature contributions while Ticket operation authority remains a host-granted backend capability, not arbitrary filesystem write scope.

## Intent

Clarify the `pod::feature` API boundary so internal built-in feature contributions are not confused with external plugin/sandbox authority grants.

The important distinction:

- Contribution declarations (`tools`, `hooks`, alerts/notices, service/background descriptors) are what a feature contributes to the host registry.
- Host authority grants are capabilities required for privileged host-mediated operations such as filesystem, network, secrets, Pod management, model notification, state store, or service access.

Internal built-in features should be able to register contributions without implying external plugin approval/sandbox semantics. External plugin authority remains a separate future concern.

## Worktree / branch

- worktree: `/home/hare/Projects/yoi/.worktree/feature-api-authority-separation`
- branch: `work/feature-api-authority-separation`

## Requirements

- Keep the change inside the existing `pod::feature` layer; do not create a new crate in this ticket.
- Make naming and type boundaries explicit enough that later built-in Ticket tools can request Ticket backend authority without being framed as an external plugin package grant.
- Prefer concrete rename/clarification over compatibility aliases because this is unreleased internal API.
- Distinguish host authority naming from generic contribution naming. Candidate renames if they fit the code:
  - `AuthorityRequest` -> `HostAuthorityRequest`
  - `AuthorityGrantSet` -> `HostAuthorityGrantSet`
  - `AuthorityDenial` -> `HostAuthorityDenial`
  - `FeatureDescriptor::requested_authorities` -> `requested_host_authorities`
  - `FeatureDescriptor::with_authority` -> `with_host_authority`
  - `ToolContribution::required_authorities` -> `required_host_authorities`
  - `FeatureInstallReport::granted_authorities` -> `host_authority_grants`
  - `FeatureInstallContext::grants()` -> `host_authority_grants()` or equivalent
- Keep contribution declarations as contribution declarations; do not require host authority grants merely to contribute a tool/hook.
- Keep existing built-in Task feature behavior unchanged.
- Keep descriptor-first validation:
  - actual contributed tools/hooks/notifications/etc. must be declared;
  - duplicate tool names remain rejected;
  - missing required host authority still fails feature installation where applicable.
- Update tests/docs/comments to reflect the new distinction.

## Non-goals

- Implementing Ticket built-in tools.
- Implementing Ticket backend authority grants beyond the generic host authority naming/model needed here.
- External plugin loading/discovery/package approval.
- WASM/sandbox runtime.
- MCP integration.
- Real human approval/resume protocol.
- Moving feature API to a new crate.
- Changing Hook API behavior.
- Changing Task feature behavior.
- Broad refactors outside `pod::feature` and direct callers/tests.

## Current code map

- `crates/pod/src/feature.rs`
  - Main feature API, descriptors, contributions, installation, authority request/grant types, tests.
  - Current generic names include `AuthorityRequest`, `AuthorityGrantSet`, `AuthorityDenial`, `requested_authorities`, `granted_authorities`, and `required_authorities`.
  - `FeatureRuntimeKind` already distinguishes `Builtin`, `LuaProfile`, and `ExternalPlugin`.
  - `HostAuthority` already names the host-mediated capability concept.
- `crates/pod/src/feature/builtin/task/mod.rs`
  - Built-in Task feature implementation and contribution example.
  - Must continue to install Task tools/hooks/reminders unchanged.
- `crates/pod/src/feature/builtin.rs`
  - Built-in feature contribution wiring.
- `crates/pod/src/lib.rs`
  - Public exports if affected.

## Critical risks

- Leaving generic `Authority*` naming in place makes later Ticket tools read like external plugin/sandbox grants, which is the confusion this ticket is meant to prevent.
- Overcorrecting by making built-in contribution registration depend on host authority grants would make internal features unnecessarily complex.
- Adding compatibility aliases for old names would preserve the ambiguity; avoid unless required by external public API, which should not be the case for this unreleased surface.
- Changing behavior instead of naming/boundary tests could destabilize Task feature installation.

## Validation

Run at least:

- `cargo test -p pod feature --lib`
- `cargo test -p pod task --lib`
- `cargo test -p pod --lib`
- `cargo test -p llm-worker --lib`
- `cargo check --workspace --all-targets`
- `cargo fmt --check`
- `git diff --check`
- `./tickets.sh doctor`

Run `nix build .#yoi --no-link` if feasible.

## Completion report

Report:

- worktree path / branch;
- commit hash;
- exact API/type/field renames;
- behavior preserved/changed;
- changed files;
- validation results;
- unresolved risks/follow-ups;
- whether `ticket-built-in-feature-tools` can proceed afterward.
