<!-- event: create author: LocalTicketBackend at: 2026-06-08T10:38:42Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: plan author: intake at: 2026-06-08T10:57:37Z -->

## Plan

## Intake refinement

readiness は `implementation_ready`。既存 Ticket の目的・受け入れ条件・除外範囲は十分に具体的で、Orchestrator が実装 routing できる。

### Binding decisions / invariants

- `legacy_ticket` と `needs_preflight` は current Ticket schema / tool input / tool output / CLI output / frontmatter writer / doctor requirements から削除する。通常の新規 Ticket 作成経路で再び出力してはいけない。
- `workflow_state: intake` は互換 alias として残さず、parser / doctor / tests で invalid として扱う。
- `workflow_state` 未指定時の bounded migration fallback を残す場合でも、`intake` 語彙を復活させてはいけない。
- `.yoi/tickets/**/item.md` の current frontmatter は stricter schema に合わせて移行する。`thread.md` / `resolution.md` の過去本文に残る語は audit history として扱い、通常は書き換えない。
- `legacy_ticket` の代替として generic external issue link field を追加しない。関係メタデータは別 Ticket `typed-ticket-relation-metadata` の範囲とする。

### Implementation latitude

- 実装者は parser / writer / metadata structs / tool schemas / panel view model / tests / prompts / docs のどこから整理するかを選んでよい。
- 移行専用の一時的な読み取り経路が必要な場合は、current API surface に露出しない bounded migration path に限定してよい。
- fixture や local Ticket record の更新方法は手動・補助スクリプトのどちらでもよいが、差分は schema 変更に必要な範囲へ限定する。

### Escalation conditions

- 非 null の `legacy_ticket` 値を削除することで保持すべき current relation 情報が失われると判断した場合は、代替フィールドを silently 追加せず Orchestrator / human に戻す。
- `needs_preflight` または `workflow_state: intake` をまだ実行時互換として保持すべき外部利用者が見つかった場合は、後方互換要件として戻す。
- 変更が Ticket schema 以外の authority boundary / role workflow semantics に広がる場合は、別 Ticket 化または routing 判断を求める。

### Validation focus

- `yoi ticket doctor` が stricter schema の local records で通ること。
- `TicketCreate` / `TicketShow` / `TicketList` / panel 表示に `legacy_ticket` と `needs_preflight` が current field として出ないこと。
- `workflow_state: intake` を invalid とする focused test を追加または更新すること。
- 既存指示どおり、実装完了時は focused tests、`cargo fmt --check`、`git diff --check`、必要に応じて `nix build .#yoi` で確認すること。

Open questions: なし。
Risk flags: `ticket-schema`, `migration`, `tool-api`, `panel-output`, `docs-prompts`.

---

<!-- event: intake_summary author: intake at: 2026-06-08T10:57:43Z -->

## Intake summary

既存 Ticket を精査し、obsolete な `legacy_ticket` / `needs_preflight` と `workflow_state: intake` 互換の削除範囲を実装可能な契約として整理した。binding decisions は current schema/tool/API/panel/docs/prompts から legacy fields を削除し、`intake` alias を拒否すること。historical thread/resolution prose は audit history として通常は保持する。Open questions はなく、risk flags は ticket-schema / migration / tool-api / panel-output / docs-prompts。

---

<!-- event: state_changed author: intake at: 2026-06-08T10:57:43Z from: planning to: ready reason: intake_ready field: workflow_state -->

## State changed

Intake refinement が完了し、Orchestrator が routing できる状態になったため `ready` に変更する。

---

<!-- event: state_changed author: workspace-panel at: 2026-06-08T11:21:33Z from: ready to: queued reason: queued field: workflow_state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: state_changed author: orchestrator at: 2026-06-08T12:10:29Z from: queued to: inprogress reason: orchestrator_acceptance field: workflow_state -->

## State changed

Accepted queued implementation after reading the Ticket, intake refinement, current workspace state, and queued Ticket set. This Ticket is a lower-level Ticket schema/API cleanup and should land before implementing the queued orchestration plan tool to avoid overlapping Ticket backend/tool API churn.

---

<!-- event: decision author: orchestrator at: 2026-06-08T12:10:29Z -->

## Decision

Routing decision: implementation_ready

Reason:
- The Ticket has clear implementation-ready requirements from Intake: remove obsolete `legacy_ticket` and `needs_preflight` current schema/API surfaces, reject `workflow_state: intake`, and migrate local Ticket records.
- The change is broad but bounded to Ticket schema/tool/API/panel/docs/tests and local Ticket record migration.
- This should run before `ticket-orchestration-plan-tool`, because the plan tool introduces a new durable Ticket-domain surface and should be implemented on top of the stricter current Ticket schema.

Evidence checked:
- Ticket body and thread, including Intake refinement and queued event.
- Open queued Ticket list: `remove-legacy-ticket-schema-fields` and `ticket-orchestration-plan-tool` are the current queued Tickets.
- Main workspace dirty state was only Ticket-record updates/new Ticket records; those have been committed separately before implementation routing.

IntentPacket:

Intent:
- Remove obsolete Ticket schema compatibility and current API exposure for `legacy_ticket`, `needs_preflight`, and `workflow_state: intake`.

Binding decisions / invariants:
- New/current Ticket frontmatter must not write `legacy_ticket` or `needs_preflight`.
- `TicketCreate`, `TicketShow`, `TicketList`, panel/view-model output, and current tool/CLI surfaces must stop exposing those fields as current metadata.
- `workflow_state: intake` must be rejected, not normalized to `planning`.
- Missing workflow_state may remain a bounded migration fallback to `planning` if needed, but must not reintroduce `intake` vocabulary.
- Existing local `.yoi/tickets/**/item.md` records must be migrated to the stricter schema; historical `thread.md` / `resolution.md` prose remains audit history unless a fixture requires explicit rewrite.
- Do not introduce a replacement generic external issue field; typed relations are out of scope and belong to `typed-ticket-relation-metadata`.
- Do not change role workflow semantics or the planning/ready/queued/inprogress/done model beyond rejecting the legacy `intake` alias.

Implementation latitude:
- Coder may choose whether migration is performed by a temporary script/manual rewrite, but the resulting diff should stay schema-focused.
- Coder may keep internal migration-only parsing helpers if strictly needed and not exposed as current API.
- Coder should update tests/fixtures/prompts/docs narrowly where they assert legacy fields or intake alias.

Escalate if:
- Any non-null `legacy_ticket` value is found whose deletion would lose current relation information.
- External/runtime compatibility requirements are found that still require `needs_preflight` or `workflow_state: intake` acceptance.
- The change requires adding typed relation metadata or broader Ticket identity redesign.

Validation:
- Focused Ticket schema/parser/writer/tool output tests.
- Focused yoi Ticket CLI/tool tests and panel/view-model tests if outputs change.
- `cargo fmt --check`.
- `git diff --check`.
- `cargo run -q -p yoi -- ticket doctor` with migrated local records.
- `cargo check --workspace` and final `nix build .#yoi` because code/tool/schema/resources may change.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-08T12:11:20Z -->

## Implementation report

Implementation routing started.

Worktree/branch:
- Worktree: `.worktree/remove-legacy-ticket-schema-fields`
- Branch: `remove-legacy-ticket-schema-fields`
- Base/routing commit: `fdb9f50 ticket: route queued schema cleanup`

Spawned sibling Coder Pod:
- `coder-remove-legacy-ticket-schema-fields`
- Scope: non-recursive read on parent workspace root plus recursive write limited to the child worktree.

The queued `ticket-orchestration-plan-tool` Ticket is intentionally left queued until this lower-level schema cleanup lands, to avoid overlapping Ticket backend/API and local Ticket-record migration surfaces.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-08T12:13:26Z -->

## Implementation report

Implementation blocked during required migration pre-check.

Coder found non-null `legacy_ticket` values before making code/docs/Ticket record edits. Per the routing packet, deletion of non-null `legacy_ticket` values must not happen silently because it may discard current relation/history information.

Summary:
- Worktree: `.worktree/remove-legacy-ticket-schema-fields`
- Branch: `remove-legacy-ticket-schema-fields`
- Code/docs/Ticket record changes: none
- Commit: none
- Validation: pre-check only
- Non-null occurrences: 32 total
  - 30 closed historical Tickets with old legacy ids
  - 2 open Tickets:
    - `.yoi/tickets/open/20260608-092854-typed-ticket-relation-metadata/item.md`: `legacy_ticket: 20260608-010000-typed-ticket-relation-metadata`
    - `.yoi/tickets/open/20260608-095529-ticket-orchestration-plan-tool/item.md`: `legacy_ticket: 20260608-020000-ticket-orchestration-plan-tool`

Needed decision before implementation continues:
- Either authorize deleting these `legacy_ticket` values as obsolete migration breadcrumbs,
- or choose a preservation/migration policy, such as copying non-null values into bounded Ticket thread migration notes before removing the field,
- or defer this Ticket back to planning to design typed relation/external-reference handling separately.

No merge, close, or implementation edits have occurred.

---

<!-- event: decision author: orchestrator at: 2026-06-08T12:17:50Z -->

## Decision

Human decision received: delete `legacy_ticket` values even when non-null.

Policy for this Ticket:
- Treat existing non-null `legacy_ticket` frontmatter values as obsolete migration breadcrumbs rather than current relation authority.
- Remove `legacy_ticket` from all current local Ticket `item.md` frontmatter, including closed historical Tickets and open Tickets.
- Do not add a replacement generic external issue field in this Ticket.
- Continue to keep historical thread/resolution prose intact unless a test fixture requires a narrow update.
- The separate `typed-ticket-relation-metadata` Ticket remains the place to design durable typed relations if needed later.

This resolves the implementation block and authorizes Coder to continue the schema cleanup.

---

<!-- event: implementation_report author: hare at: 2026-06-08T12:36:29Z -->

## Implementation report

Implemented legacy Ticket schema field removal.

- Removed current schema/API exposure for `legacy_ticket` and `needs_preflight` from Ticket metadata, summaries, tool output, and create inputs.
- Rejected `workflow_state: intake` instead of treating it as a planning alias.
- Migrated current local Ticket `item.md` frontmatter to remove obsolete `legacy_ticket` / `needs_preflight` entries, including non-null migration breadcrumbs.
- Updated focused tests for create/list/show/tool output and panel metadata behavior.

Validation completed before this report:
- `cargo test -p ticket --lib`
- `cargo test -p yoi ticket_cli`
- `cargo test -p tui workspace_panel`
- `cargo fmt --check`
- `git diff --check`
- `cargo run -q -p yoi -- ticket doctor`
- `cargo check --workspace`
- `nix build .#yoi`


---
