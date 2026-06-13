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

<!-- event: implementation_report author: orchestrator at: 2026-06-13T18:42:45Z -->

## Implementation report

Implementation handoff started.

- Worktree: `/home/hare/Projects/yoi/.worktree/remove-feature-hostauthority`
- Branch: `ticket-00001KV0SP0TY-remove-feature-hostauthority`
- Coder Pod: `coder-00001KV0SP0TY-hostauthority`
- Scope: root workspace non-recursive read for launch validation only; child worktree read; writes limited to relevant feature/Ticket/tool/client/workflow/manifest/docs surfaces, target, and this Ticket record。

Next: Coder implementation_report / commit / validation evidence を確認後、Reviewer を起動する。

---

<!-- event: implementation_report author: hare at: 2026-06-13T18:53:02Z -->

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

<!-- event: review author: hare at: 2026-06-13T18:58:42Z status: approve -->

## Review: approve

Reviewed implementation commits `5549c50d` and `3faf7d7b` against Ticket `00001KV0SP0TY`.

Evidence:
- `crates/pod/src/feature.rs` removes the feature-layer `HostAuthority`, `HostAuthorityRequest`, `HostAuthorityGrantSet`, grant/denial/install-report semantics, and per-tool authority gates. The remaining feature API is contribution/lifecycle/diagnostic oriented: tools, hooks, background tasks, services, notifications, and install diagnostics.
- No renamed equivalent authority/grant layer was introduced in `pod::feature`; remaining checks are ordinary contribution diagnostics such as duplicate tool names, undeclared tools/hooks/background tasks/services, service requirements, skipped contributions, and install errors.
- `crates/pod/src/feature/builtin/ticket.rs` no longer requests or registers feature-layer host authority. Ticket tool exposure remains controlled by `TicketFeatureConfig` resolution, backend provider/root validation, and `TicketFeatureAccess` / base-orchestration tool selection. Missing/invalid backend roots and unsupported providers still fail closed without registering tools.
- `crates/pod/src/feature/builtin/task/mod.rs` no longer describes task tools in host-authority terms; task hooks/tools still install through the normal feature registry path.
- Source grep under `crates/**/*.rs` found no remaining `HostAuthority`, `HostAuthorityRequest`, `HostAuthorityGrantSet`, `host_authority`, `with_host_authority`, `requested_host_authorities`, or `required_host_authorities` symbols/terms.
- Related Plugin/MCP planning Tickets inspected (`00001KTR81P9X`, `00001KTR82RB7`, `00001KSXRQ4G8`, `00001KT0Z4BK8`) state that Plugin/MCP permission/trust policy is not a `pod::feature` authority/grant dependency. The remaining mentions are negative boundary statements, not implementation reliance.

Validation performed:
- `git diff --check c4465a04..HEAD` — passed.
- `cargo test -p pod feature:: --lib` — passed.
- `cargo test -p pod feature::builtin::task --lib` — passed.
- `cargo test -p ticket` — passed.
- `cargo fmt --check` — passed.
- `cargo check --workspace --all-targets` — passed.

Not run:
- `nix build .#yoi`; skipped because this review scope is source read-only with writes limited to `target/` and this Ticket record, and the change does not touch dependencies, `Cargo.lock`, packaging, or source filters.

Risks / notes:
- This intentionally removes the feature-layer authority gate rather than replacing it. Any future Plugin/MCP permission policy must be implemented in its owning layer and must not reintroduce this as a feature-layer grant API.

Decision: approved.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-13T19:01:55Z -->

## Implementation report

Integration outcome: merged and validated.

- Implementation branch: `ticket-00001KV0SP0TY-remove-feature-hostauthority`
- Implementation commits: `5549c50d feat: remove feature host authority API`, `3faf7d7b ticket: report host authority cleanup`
- Review commit: `92e64bda ticket: approve host authority cleanup`
- Orchestrator merge commit: `297e95ef merge: remove feature host authority`

Reviewer result:
- `approve`。feature-layer `HostAuthority` / `HostAuthorityRequest` / `HostAuthorityGrantSet`、grant/denial/install-report semantics、per-tool authority gates が削除され、renamed replacement authority layer は導入されていないことを確認済み。

Orchestrator validation after merge:
- `cargo test -p pod feature:: --lib`: PASS
- `cargo test -p pod feature::builtin::task --lib`: PASS
- `cargo test -p ticket`: PASS
- `cargo fmt --check`: PASS
- `git diff --check`: PASS
- `cargo check --workspace --all-targets`: first attempt failed due to host disk full (`No space left on device`); after stopping HostAuthority child Pods and removing their child worktree/target, rerun PASS。

Cleanup performed:
- stopped `coder-00001KV0SP0TY-hostauthority` and `reviewer-00001KV0SP0TY-hostauthority`
- removed child worktree `/home/hare/Projects/yoi/.worktree/remove-feature-hostauthority`
- deleted branch `ticket-00001KV0SP0TY-remove-feature-hostauthority`

Not run:
- `nix build .#yoi`; skipped because dependencies / `Cargo.lock` / packaging/source filters were not changed and disk pressure was encountered during validation。

Next:
- mark Ticket done. Closure remains separate.

---

<!-- event: state_changed author: orchestrator at: 2026-06-13T19:02:01Z from: inprogress to: done reason: merged_and_validated field: state -->

## State changed

Implementation branch was reviewed, approved, merged into the Orchestrator branch as `297e95ef`, and validated in the Orchestrator worktree. Focused pod/ticket tests, formatting, diff check, and `cargo check --workspace --all-targets` passed after cleanup freed disk space. Ticket implementation work is done; closure remains separate.

---
