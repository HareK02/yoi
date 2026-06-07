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

<!-- event: implementation_report author: orchestrator at: 2026-06-07T23:14:08Z -->

## Implementation report

Coder completed implementation:
- Pod: `coder-parse-ticket-frontmatter-as-yaml`
- Branch/worktree: `ticket/parse-ticket-frontmatter-as-yaml` at `.worktree/parse-ticket-frontmatter-as-yaml`
- Commit: `10d7844fc84683429e24671bf01dd57885a40084` (`ticket: parse item frontmatter as YAML`)
- Child worktree status: clean

Implementation summary:
- Replaced raw Ticket `item.md` frontmatter parsing with strict YAML parsing in `crates/ticket/src/lib.rs`.
- Added typed handling for YAML nulls, sequences, booleans, quoted string values, and clear parse/type errors.
- Removed tolerant raw-string fallbacks that conflict with YAML semantics.
- Added focused parser tests and workspace panel regression coverage for `workflow_state: intake` plus `attention_required: null` deriving `Clarify`, not `Edit`.
- Updated `crates/ticket/Cargo.toml`, `Cargo.lock`, and `package.nix` for `serde_yaml` / Nix cargo hash.
- Migrated 48 existing `.yoi/tickets/**/item.md` files by quoting YAML-unsafe title values with colons; no bodies, threads, resolutions, or non-frontmatter content were rewritten.

Reported validation:
- `cargo test -p ticket --lib` — passed
- `cargo test -p tui workspace_panel --lib` — passed
- `cargo fmt --check` — passed
- `git diff --check` — passed
- `target/debug/yoi ticket doctor` — passed
- `nix build .#yoi` — passed

Next action:
- Delegate independent read-only reviewer inspection of commit `10d7844` against the Ticket intent, strict-YAML preflight boundary, migration minimality, and validation evidence before preparing a merge-ready dossier.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-07T23:14:34Z -->

## Implementation report

Reviewer delegated:
- Pod: `reviewer-parse-ticket-frontmatter-as-yaml`
- Review target: commit `10d7844` on branch `ticket/parse-ticket-frontmatter-as-yaml`
- Scope: read-only child worktree plus git metadata needed to inspect the branch.
- Instructions: inspect parser semantics, clear failure behavior, minimal Ticket record migration, workspace panel regression coverage, dependency/package changes, and validation evidence; return `approve` or `request_changes` with blocker/non-blocker separation. No file edits, Ticket writes, merge, close, or cleanup delegated.

---

<!-- event: review author: reviewer-parse-ticket-frontmatter-as-yaml at: 2026-06-07T23:18:48Z status: request_changes -->

## Review: request changes

Branch-local reviewer verdict: request changes.

Blocker:
- The parser now correctly treats schema string fields as YAML strings, but the Ticket frontmatter writer can still emit numeric-looking or otherwise ambiguous string values as plain YAML scalars. Examples such as `title: 123`, `labels: [123]`, `risk_flags: [1]`, or `assignee: 42` would be emitted as non-string YAML scalars and then rejected by the strict parser on the next read. The backend must not create/update Ticket records that its own parser rejects.

Requested fix:
- Make string serialization conservative, preferably always quote schema string values, or prove round-trip string-ness under `serde_yaml` before emitting plain scalars.
- Add regression tests for numeric-looking strings at least for `title`, `labels`, and `risk_flags`; optional string fields such as `assignee`/`attention_required` should also be covered if they share the helper.

Positive findings:
- Strict YAML parsing direction is correct.
- Null/list/bool parsing and clear failure behavior are covered.
- Workspace panel regression coverage is present.
- Migration appears limited to necessary `title:` frontmatter normalization.
- Dependency/package changes looked coherent and coder-reported validation was appropriate.

Validation expected after fix:
- `cargo test -p ticket --lib`
- `cargo test -p tui workspace_panel --lib`
- `target/debug/yoi ticket doctor` or equivalent
- `nix build .#yoi` if dependency/package outputs remain changed.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-07T23:22:43Z -->

## Implementation report

Coder completed reviewer-blocker fix:
- Pod: `coder-parse-ticket-frontmatter-as-yaml`
- Branch/worktree: `ticket/parse-ticket-frontmatter-as-yaml` at `.worktree/parse-ticket-frontmatter-as-yaml`
- New commit: `dbdccc5 ticket: quote frontmatter strings conservatively`
- Branch commits now: `10d7844` + `dbdccc5`
- Child worktree status: clean

Fix summary:
- `format_yaml_string_scalar` now emits schema string values as single-quoted YAML scalars conservatively.
- Applies to scalar string fields such as `title`, `assignee`, `attention_required`, list items in `labels`/`risk_flags`, and string updates through `set_frontmatter_fields`.
- Added regression coverage for ambiguous/numeric-looking user strings: `title: "123"`, `labels: ["123", "01"]`, `risk_flags: ["1", "42"]`, `assignee: "42"`, `attention_required: "0"`, and `action_required: "true"`.
- No extra Ticket migration rewrite was needed for the fix.

Reported validation after fix:
- `cargo test -p ticket --lib` — passed
- `cargo test -p tui workspace_panel --lib` — passed
- `cargo fmt --check` — passed
- `git diff --check` — passed
- `target/debug/yoi ticket doctor` — passed
- `nix build .#yoi` — passed

Next action:
- Send the updated branch back to the reviewer for focused re-review of the prior blocker and final merge-ready verdict.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-07T23:25:23Z -->

## Implementation report

Merge-ready dossier for `orchestrator-merge-completion`

Ticket:
- Id: `20260607-223233-parse-ticket-frontmatter-as-yaml`
- Slug: `parse-ticket-frontmatter-as-yaml`
- Workflow state: `inprogress`

Branch/worktree:
- Branch: `ticket/parse-ticket-frontmatter-as-yaml`
- Worktree: `.worktree/parse-ticket-frontmatter-as-yaml`
- Child worktree status: clean (`## ticket/parse-ticket-frontmatter-as-yaml`)

Commits:
- `10d7844 ticket: parse item frontmatter as YAML`
- `dbdccc5 ticket: quote frontmatter strings conservatively`
- Diff vs `develop`: 53 files changed; source/package changes in `crates/ticket`, `crates/tui/src/workspace_panel.rs`, `Cargo.lock`, `package.nix`; 48 `.yoi/tickets/**/item.md` records minimally normalized.

Intent/invariant check:
- Intent satisfied: Ticket `item.md` frontmatter now parses through strict YAML-compatible typed metadata rather than raw `key: value` strings.
- Human/preflight boundary followed: old tolerant raw-string semantics that conflict with YAML were not preserved.
- YAML null/list/bool semantics are handled; invalid/wrong-typed frontmatter fails clearly.
- Writer/parser round-trip invariant restored by conservative quoting of schema string values.
- Scope stayed within parser/emitter correctness, minimal project Ticket record normalization, package dependency updates, and workspace panel regression coverage; Ticket lifecycle/state/action model was not redesigned.

Implementation summary:
- Replaced raw frontmatter parsing in `crates/ticket/src/lib.rs` with `serde_yaml::Value`-based strict parsing and typed helpers.
- Added validation/error paths for wrong YAML field types.
- Added tests for null values, quoted `"null"` as string where valid, list fields, bool fields, invalid types, and conservative string emission.
- Added workspace panel regression coverage so `workflow_state: intake` plus `attention_required: null` derives `Clarify` rather than `Edit`.
- Added `serde_yaml` to `crates/ticket/Cargo.toml`, updated `Cargo.lock`, and updated `package.nix` cargo hash.
- Normalized existing Ticket item frontmatter by quoting YAML-unsafe `title:` values containing colons; no body/thread/resolution rewrite was introduced.

Coder evidence:
- Pod: `coder-parse-ticket-frontmatter-as-yaml`
- Initial implementation commit: `10d7844`
- Reviewer-fix commit: `dbdccc5`
- Coder-reported final status: child worktree clean.
- Coder-reported validation after fix:
  - `cargo test -p ticket --lib` — passed
  - `cargo test -p tui workspace_panel --lib` — passed
  - `cargo fmt --check` — passed
  - `git diff --check` — passed
  - `target/debug/yoi ticket doctor` — passed
  - `nix build .#yoi` — passed

Reviewer evidence:
- Pod: `reviewer-parse-ticket-frontmatter-as-yaml`
- First verdict: request_changes due writer emitting ambiguous numeric-looking schema strings as plain YAML scalars.
- Blocker resolution: reviewer approved after focused re-review of commit `dbdccc5`.
- Reviewer inspected branch/worktree state, prior-to-fix delta, focused `crates/ticket/src/lib.rs` diff, migration scope, packaging/dependency diff, `git diff --check`, and `target/debug/yoi ticket doctor`.
- Final branch-local verdict: approve.

Blockers / findings:
- Resolved blocker: strict parser could reject records written by the backend if schema strings were emitted as ambiguous plain YAML scalars.
- Fix: schema string values are conservatively single-quoted, including list entries and update paths; regression tests cover numeric/ambiguous strings.
- No remaining reviewer blockers or non-blocker required changes.

Validation performed by orchestrator:
- Confirmed child branch HEAD, clean status, commit list, and diff stat/name-status against `develop`.
- Ran typed `TicketDoctor`: 0 errors; existing repository-wide warnings remain unrelated/truncated.

Residual risks:
- `serde_yaml` dependency/package change is larger than parser-only code but was validated by coder-reported `nix build .#yoi` and reviewer packaging inspection.
- Ticket migration touches many item files, but the diff is intentionally limited to frontmatter title quoting needed for valid YAML parsing.
- Broader historical Ticket warnings remain but are unrelated to this change.

Current main workspace dirty state:
- Main branch has local Ticket record modifications for this Ticket's routing/review/dossier events.
- No source-code changes are present in the main workspace outside child worktrees.

Pods / cleanup state:
- Coder Pod is idle/reachable.
- Reviewer Pod is idle/reachable.
- Worktree and branch are retained for merge-completion.
- No merge, final main-branch approval, close, or cleanup has been performed.

Parent/human decision needed:
- Authorize merge-completion if desired. Without explicit merge authority, stop here with this dossier.
- Merge-completion should independently verify branch/worktree/commit identity, main workspace safety, and validation requirements before merge/close/cleanup.

---
