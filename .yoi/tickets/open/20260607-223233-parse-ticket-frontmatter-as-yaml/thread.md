<!-- event: create author: LocalTicketBackend at: 2026-06-07T22:32:33Z -->

## Created

Created by LocalTicketBackend create.

---

<!-- event: intake_summary author: intake at: 2026-06-07T22:36:44Z -->

## Intake summary

Existing Ticket is sufficiently specified and not a duplicate of another visible active Ticket. Intake classifies it as implementation_ready with needs_preflight=true because it changes durable Ticket frontmatter parsing and compatibility semantics. Risk flags: ticket-backend, yaml-parsing, compatibility, panel-regression. Scope is limited to YAML/null-aware Ticket item.md parsing, list/bool handling, preserving emitted format/doctor compatibility, and the workspace panel attention_required:null regression. Validation should include focused ticket parser tests, workspace_panel regression tests, cargo fmt --check, git diff --check, and relevant cargo test commands; final implementation should also consider the project-level nix build requirement.

---

<!-- event: state_changed author: intake at: 2026-06-07T22:36:44Z from: intake to: ready reason: intake_ready field: workflow_state -->

## State changed

Intake completed: requirements, acceptance criteria, non-goals, affected fields, and validation are explicit enough for Orchestrator routing. Human queueing remains required before implementation starts.

---

<!-- event: state_changed author: workspace-panel at: 2026-06-07T22:43:03Z from: ready to: queued reason: queued field: workflow_state -->

## State changed

Ticket queued for Orchestrator routing.


---

<!-- event: decision author: orchestrator at: 2026-06-07T22:43:24Z -->

## Decision

Routing decision: preflight_needed

Reason:
- The Ticket is well scoped and urgent, but it changes durable Ticket frontmatter parsing and compatibility semantics.
- Intake explicitly classified it as implementation-ready only with `needs_preflight=true`, with risk flags `ticket-backend`, `yaml-parsing`, `compatibility`, and `panel-regression` in the intake summary.
- YAML/null-aware parsing affects nullable strings, lists, booleans, doctor compatibility, existing fixture behavior, and workspace panel action derivation; a short design boundary check should precede coder delegation.

Evidence checked:
- Ticket body requirements, candidate affected fields, non-goals, and acceptance criteria.
- Intake summary and `ready -> queued` event.
- Workspace state: `develop` has only this Ticket's queued item/thread modifications plus an unrelated untracked Ticket directory `20260607-224309-reduce-ticket-lifecycle-commit-noise`; no branch/worktree exists for this slug.
- Visible Pods: intake peer for this Ticket is idle; no coder/reviewer Pods are active for this Ticket.

Next action:
- Run `ticket-preflight-workflow` before implementation delegation.
- Preflight should decide the parsing boundary: whether to deserialize `item.md` frontmatter through `serde_yaml::Value`/typed helpers or a narrower YAML-compatible layer; how to preserve emitted format; how to represent quoted literal `"null"` versus YAML null; and which tests cover Ticket parser plus workspace panel regression.
- Do not transition `queued -> inprogress`, create `.worktree/parse-ticket-frontmatter-as-yaml`, or spawn coder/reviewer Pods until preflight records implementation readiness.

Escalate if:
- Existing Ticket records rely on raw-string parsing semantics that conflict with YAML behavior.
- Adding a YAML parser dependency has packaging/licensing implications not already acceptable for this crate graph.
- Fixing panel action derivation requires broader Ticket workflow/action model changes beyond parsing correctness.

---

<!-- event: decision author: preflight at: 2026-06-07T22:55:52Z -->

## Decision

Preflight decision: implement a YAML-aware typed parsing layer for Ticket `item.md` frontmatter values, not a strict whole-frontmatter YAML deserialization/migration.

Implementation boundary:
- Keep `parse_item` responsible for splitting `---` frontmatter from Markdown body and preserving `raw_frontmatter: BTreeMap<String, String>` for compatibility/debug surfaces.
- Add `serde_yaml` to `crates/ticket` and parse each scalar value through helper functions rather than replacing the whole frontmatter parser.
- Nullable string helpers must treat YAML null forms (`null`, `Null`, `NULL`, `~`, empty value) as `None`, while preserving quoted literal strings such as `"null"` as meaningful text where applicable.
- List helpers should support YAML sequences for `labels` / `risk_flags` and preserve existing simple inline list behavior.
- Boolean helpers should support YAML booleans and keep the existing tolerant `yes/no/1/0` behavior.
- Do not redesign the Ticket schema, migrate records, or change emitted frontmatter formatting in this Ticket.

Rationale:
- Existing Ticket frontmatter is YAML-like but may contain unquoted string values that are safer to keep under tolerant field-level parsing than to force through strict whole-document YAML semantics.
- The reported regression is caused by typed optional/list/bool interpretation, so the fix should target typed field extraction while preserving current storage/writer compatibility.

Required tests:
- `attention_required: null`, `~`, and empty value parse as `None`.
- quoted `"null"` remains distinguishable from YAML null for optional string fields.
- `labels` / `risk_flags` YAML sequences parse correctly.
- `needs_preflight: true` parses as `Some(true)` and tolerant legacy bool forms still work.
- Workspace panel regression: `workflow_state: intake` plus `attention_required: null` derives `Clarify`, not `Edit`.

Validation target remains the Ticket acceptance criteria plus project packaging validation where implementation touches source/package dependencies.

---

<!-- event: decision author: preflight at: 2026-06-07T22:57:35Z -->

## Decision

Preflight decision revised by human direction: do not preserve the old raw-string/tolerant Ticket frontmatter parsing semantics. Intentionally remove that backward compatibility and migrate this project's existing `.yoi/tickets` records to the new strict YAML-compatible format as part of the implementation.

Supersedes the previous preflight recommendation to keep a field-level tolerant compatibility layer.

Updated implementation boundary:
- Parse `item.md` frontmatter as real YAML data instead of the current raw `key: value` string map.
- Introduce typed YAML deserialization/parsing for Ticket metadata and fail clearly on invalid frontmatter rather than silently preserving legacy raw-string behavior.
- Add a repository-local migration for the existing `.yoi/tickets` records in this project so current records are valid under the new parser.
- Preserve emitted frontmatter format only insofar as it is valid YAML; quote/normalize existing values where needed during migration.
- Remove per-field ad-hoc `"null"` filtering and old tolerant list/string compatibility when it conflicts with YAML semantics.
- Keep the Ticket schema scope focused; do not redesign Ticket lifecycle or panel action model.

Updated tests/validation:
- Parser tests should assert YAML null/list/bool semantics directly.
- Tests should cover invalid frontmatter failure where appropriate instead of legacy fallback acceptance.
- Workspace panel regression remains required: `workflow_state: intake` plus `attention_required: null` derives `Clarify`, not `Edit`.
- Migration diff should include only the needed `.yoi/tickets` frontmatter normalization, not unrelated Ticket content rewrites.

---

<!-- event: decision author: orchestrator at: 2026-06-07T22:59:03Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Preflight has now been recorded and revised by human direction.
- The design boundary is explicit: replace raw-string/tolerant Ticket item frontmatter parsing with real YAML-compatible typed parsing, fail clearly on invalid frontmatter, and migrate this project's existing `.yoi/tickets` records to valid YAML where needed.
- The scope is still bounded to Ticket metadata parsing/emission compatibility, project-local Ticket record normalization, and workspace panel regression coverage; lifecycle and panel action model redesign remain non-goals.

Evidence checked:
- Ticket body requirements and acceptance criteria.
- Intake summary and previous routing decision.
- Preflight decision at `2026-06-07T22:55:52Z` and human revision at `2026-06-07T22:57:35Z`.
- Workspace state: no branch/worktree exists for this slug; unrelated untracked Ticket directories are present but do not alter the source tree or the child worktree branch point.
- Visible Pods: intake peer is idle; no coder/reviewer Pods are active for this Ticket.

IntentPacket:

Intent:
- Parse Ticket `item.md` frontmatter as real YAML-compatible typed metadata so YAML null/list/bool semantics work correctly, including workspace panel action derivation.

Requirements:
- Replace the current raw `key: value` string interpretation for Ticket item frontmatter with typed YAML parsing/deserialization.
- Ensure YAML null values such as `null`, `~`, and empty nullable fields become `None` where the schema field is optional.
- Preserve quoted string semantics: quoted `"null"` is a string, not null, when valid for that field.
- Parse YAML sequences for list fields such as `labels` and `risk_flags`.
- Parse YAML booleans directly; remove old tolerant raw-string compatibility where it conflicts with YAML semantics.
- Fail clearly on invalid frontmatter instead of silently falling back to legacy raw-string behavior.
- Migrate this repository's existing `.yoi/tickets` records so their frontmatter is valid for the new parser, with minimal normalization.
- Add/adjust focused tests for parser semantics and the workspace panel regression: `workflow_state: intake` plus `attention_required: null` derives `Clarify`, not `Edit`.

Invariants:
- Do not redesign Ticket lifecycle, workflow states, panel action model, or Ticket schema beyond parser correctness.
- Keep migration diff limited to necessary `.yoi/tickets` frontmatter normalization; avoid unrelated Ticket body/thread rewrites.
- Preserve generated/runtime/local/lock/secret-like `.yoi` boundaries; do not edit `.yoi/memory` or `.yoi/**/_logs/**`.
- Child worktree must not edit main-workspace Ticket/workflow/docs records directly outside its delegated branch.

Non-goals:
- No external tracker integration.
- No broad lifecycle/action-required redesign.
- No compatibility layer that preserves invalid legacy raw-string semantics against YAML behavior.

Escalate if:
- Existing project Ticket records cannot be normalized without semantic ambiguity.
- YAML parsing requires a dependency or package change that fails licensing/packaging validation.
- Workspace panel regression fix requires changing action derivation semantics beyond correct typed metadata.

Validation:
- Focused `cargo test` for `crates/ticket` parser/doctor tests touched by the change.
- Focused workspace panel test covering the `attention_required: null` regression.
- `cargo fmt --check`.
- `git diff --check`.
- `target/debug/yoi ticket doctor` or equivalent built binary doctor.
- `nix build .#yoi` because source/dependency/package behavior may change.

Current code map:
- `crates/ticket` local backend/item parser/frontmatter rendering/tests.
- `crates/tui` workspace panel action derivation tests only if needed for regression coverage.
- Existing `.yoi/tickets` records for minimal YAML normalization.

Critical risks:
- Accidentally treating quoted string values as null.
- Over-normalizing Ticket records and creating noisy lifecycle/thread diffs.
- Breaking Ticket doctor/list/show/panel reads on existing records.
- Adding `serde_yaml` without updating Cargo/Nix package expectations.

---

<!-- event: state_changed author: orchestrator at: 2026-06-07T22:59:08Z from: queued to: inprogress reason: orchestrator_acceptance_after_preflight field: workflow_state -->

## State changed

Orchestrator accepted the queued Ticket for implementation after preflight recorded the strict YAML parsing and project-local migration boundary. Worktree and sibling coder/reviewer routing may proceed from this accepted in-progress state.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-07T22:59:45Z -->

## Implementation report

Coder delegated:
- Pod: `coder-parse-ticket-frontmatter-as-yaml`
- Worktree: `.worktree/parse-ticket-frontmatter-as-yaml`
- Branch: `ticket/parse-ticket-frontmatter-as-yaml`
- Scope: read access to main workspace only for startup/cwd; write access to the child worktree.
- Instructions: implement strict YAML-compatible Ticket item frontmatter parsing, migrate existing project Ticket records minimally, add parser and workspace panel regression tests, run focused validation plus `cargo fmt --check`, `git diff --check`, Ticket doctor, and `nix build .#yoi`, then commit locally on the child branch. No merge, close, main workspace edits, or cleanup delegated.

---
