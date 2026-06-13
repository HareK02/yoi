<!-- event: create author: "yoi ticket" at: 2026-06-13T15:30:22Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: decision author: hare at: 2026-06-13T16:08:36Z -->

## Decision

決定:
- feature-layer `HostAuthority` は残さず削除する。
- rename して診断用に温存する案も採用しない。`Authority` / `Grant` という語彙を `pod::feature` public API に残すと、Plugin/MCP permission layer と再び混同されるため。
- feature install report に必要な情報は ordinary diagnostics / skipped contribution reason として表現する。


---

<!-- event: intake_summary author: hare at: 2026-06-13T16:27:15Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-13T16:27:15Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-13T16:33:15Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-06-13T18:41:14Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Ticket は `queued` で、feature-layer `HostAuthority*` model を削除し、permission/trust は Plugin/MCP/Ticket/tool owning layers に置く decision が明確。
- `TicketRelationQuery` には incoming `depends_on`（`00001KTR81P9X` がこの Ticket に依存）があるが、この Ticket 自身を blocking する relation はない。むしろ後続 dynamic provider work の前提として先に進めるべき。
- `TicketOrchestrationPlanQuery` に blocker はない。
- Risk は feature-api / tool-registry / ticket-tools だが、削除対象・非目標（Plugin/MCP permission policy を導入しない）が明記済み。
- 他 queued work と主な変更面が異なるため並列開始可能。API cleanup の影響が大きい場合は Coder に escalation させる。

Evidence checked:
- Ticket body / thread / artifacts。
- relation records: incoming dependency from `00001KTR81P9X` only。
- orchestration plan records: なし。
- bounded code context: `HostAuthority*` occurrences across `crates/pod` / feature registry / ticket/task feature tests。

IntentPacket:
- `pod::feature` public API から `HostAuthority`, `HostAuthorityRequest`, `HostAuthorityGrantSet` と related grant/install report semantics を削除し、ordinary contribution diagnostics に整理する。

Binding decisions / invariants:
- Renamed feature-layer authority/grant model は作らない。
- Plugin permission policy / MCP trust policy はこの Ticket で実装しない。
- Ticket tools は explicit TicketFeatureConfig / backend validation / access-level config を維持する。
- Useful contribution checks（duplicate names、undeclared categories等）は残す。

Validation:
- Focused pod feature / ticket / task tests、`cargo fmt --check`、`cargo check --workspace --all-targets`、可能なら `nix build .#yoi`。

---

<!-- event: state_changed author: orchestrator at: 2026-06-13T18:41:25Z from: queued to: inprogress reason: orchestrator_acceptance_unblocked field: state -->

## State changed

Ticket evidence、relation records、orchestration plan、workspace state を確認した。This Ticket 自身を blocking する relation はなく、incoming dependent Ticket の前提として先に進めるべき。実装 side effect の前に inprogress acceptance を記録する。

---

<!-- event: implementation_report author: hare at: 2026-06-13T18:53:02Z -->

## Implementation report

## Implementation report

Commit: `5549c50d8679` (`feat: remove feature host authority API`)

Files changed:
- `crates/pod/src/feature.rs`
  - Removed `HostAuthority`, `HostAuthorityRequest`, `HostAuthorityGrantSet`, host-authority denial errors, grant plumbing, and per-tool host-authority gates from the public feature API/install path.
  - Kept ordinary contribution declaration diagnostics: duplicate names, undeclared/mismatched tools, undeclared hooks/background tasks/services, skipped contributions, and service requirement diagnostics.
  - Kept install reports focused on installed/skipped contributions and diagnostics without security grant semantics.
- `crates/pod/src/feature/builtin/ticket.rs`
  - Stopped declaring TicketBackend host authority and registering Ticket tools with feature-layer authority requirements.
  - Preserved explicit `TicketFeatureConfig`, backend provider/root validation, and access-level-controlled tool exposure.
- `crates/pod/src/feature/builtin/task/mod.rs`
  - Removed Task feature prose that referenced feature-layer host authorities.

Validation:
- `cargo test -p pod feature:: --lib` — passed (46 tests).
- `cargo test -p pod feature::builtin::task --lib` — passed (18 tests).
- `cargo test -p ticket` — passed (68 tests + doc-tests).
- `cargo fmt --check` — passed.
- `cargo check --workspace --all-targets` — passed.
- `git diff --check` — passed before commit.
- Source grep under `crates/**/*.rs` found no remaining `HostAuthority*`, `host_authority`, `with_host_authority`, `requested_host_authorities`, or `required_host_authorities` symbols/terminology after cleanup.

Residual risks / notes:
- `nix build .#yoi` was not run; this change does not modify dependencies, `Cargo.lock`, packaging, or source filters.
- Related Plugin/MCP planning Ticket review was not performed from this delegated Coder scope because only Ticket `00001KV0SP0TY` is readable/writable in `.yoi/tickets`; no Plugin/MCP permission or trust policy was implemented.


---
