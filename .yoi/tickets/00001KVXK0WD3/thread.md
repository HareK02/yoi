<!-- event: create author: "yoi ticket" at: 2026-06-24T19:51:56Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-06-24T19:55:29Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-24T19:55:29Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-24T20:11:56Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-24T20:13:18Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Dashboard Queue により人間が Orchestrator routing を許可した queued Ticket として確認した。
- Ticket body は legacy raw `wasm` runtime redesign のうち最初の concrete slice として、Pod runtime 内の `PluginInstanceRuntime::LegacyToolAdapter` 相当 active execution path を削除し、`wasm-component` / Component Model path を唯一の active runtime path にする範囲に限定している。
- Manifest / CLI diagnostics rejection、Service / Ingress event queue、WebSocket driver、WIT / PDK / templates は明示的に non-goal / 後続 Ticket に分割されている。
- `TicketRelationQuery` は 1 件で、この Ticket は後続 Ticket から参照される dependency chain の先頭であり、blocking outgoing dependency はない。
- `TicketOrchestrationPlanQuery` は routing 前 plan 0 件。accepted plan `orch-plan-20260624-201247-1` を記録済み。
- bounded context check で `crates/pod/src/feature/plugin.rs` の `LegacyToolAdapter` / raw `wasm` active path、`crates/manifest/src/plugin.rs` と `crates/yoi/src/plugin_cli.rs` の legacy manifest/diagnostic fixtures、docs の transitional runtime 記述を確認した。Ticket は manifest rejection を後続 Ticket に分けているため、残る不確実性は local implementation / test update に収まる。

Evidence checked:
- Ticket body / thread: `item.md`, `thread.md`。thread は create、planning->ready、ready->queued のみで未解決 blocker は記録されていない。
- Relations / orchestration plan: relation 1 件（後続 Ticket がこの Ticket に依存する view）、routing 前 plan 0 件。
- Code/docs context: `crates/pod/src/feature/plugin.rs` の `LegacyToolAdapter` / raw WASM handling、`crates/manifest/src/plugin.rs` / `crates/yoi/src/plugin_cli.rs` の legacy runtime constants/tests、Plugin docs の current transition notes。
- Workspace state: `/home/hare/Projects/yoi/.worktree/orchestration` は clean。inprogress Ticket は 0 件。
- Queue context: 他の queued Plugin follow-up Tickets は dependency chain 上でこの Ticket の完了後に進める。

IntentPacket:

Intent:
- Pod Plugin runtime の active execution path から legacy raw core-WASM Tool adapter を削除し、Plugin Tool execution を Component Model runtime path に一本化する。

Binding decisions / invariants:
- この Ticket では active runtime execution path を整理する。Manifest / CLI の外向き rejection UX は後続 `00001KVXK0WDH` の範囲として残す。
- Component Model Plugin Tool execution、Host API grant boundary、package discovery、enablement、digest pinning、ToolRegistry registration は維持する。
- raw core-WASM path を compatibility fallback として active execution に残さない。
- Service / Ingress event runtime、WebSocket driver、WIT/PDK/templates service event update は実装しない。
- broad Plugin redesign や public registry/install/update policy に範囲を広げない。

Requirements / acceptance criteria:
- `PluginInstanceRuntime::LegacyToolAdapter` または同等の legacy raw-WASM adapter が active runtime から削除される。
- raw `wasm` runtime を通じた Plugin Tool execution path が使われない。
- Component Model Plugin Tool execution tests が通る。
- Legacy runtime 削除に伴う dead code / dead tests / obsolete fixtures が整理される。
- Discovery / enablement / Tool registration / grant validation の既存挙動が壊れない。

Implementation latitude:
- `PluginInstanceRuntime` enum の shape を単一 component path に畳むか、名前を残して variant を整理するかは既存 code style に合わせてよい。
- Manifest layer の legacy constants/tests は、active runtime removal に必要な最小限だけ調整してよい。ただし user-facing rejection/diagnostic completion は後続 Ticket に残す。
- Docs の compatibility/transitional wording は、active runtime removal と矛盾する部分を最小限更新してよい。

Escalate if:
- Component Model path だけでは既存 Tool execution / grant validation / ToolRegistry registration を維持できない。
- Manifest/CLI outward rejection を同時実装しないと build/test が成立しない。
- Wasmtime/component runtime limits or sandbox boundaries を再設計する必要がある。
- Service/Ingress/WebSocket/WIT/PDK の設計変更が必要になる。

Validation:
- `cargo test -p pod`
- `cargo check -p yoi`
- `git diff --check`
- `nix build .#yoi --no-link`
- 必要に応じて focused Plugin tests / `cargo test -p manifest` / `cargo test -p yoi plugin_cli`。

Current code/docs map:
- Primary: `crates/pod/src/feature/plugin.rs`。
- Secondary: `crates/manifest/src/plugin.rs`, `crates/yoi/src/plugin_cli.rs`, Plugin docs/templates only as needed for active runtime removal consistency。
- Avoid: full service runtime, WebSocket driver, WIT/PDK event model, remote plugin registry, root/original workspace operations。

Critical risks / reviewer focus:
- legacy raw-WASM execution path accidentally remains as fallback。
- Component Model Tool execution regression。
- host API grant / digest pinning / enablement / ToolRegistry registration regression。
- scope creep into manifest rejection or Service/Ingress runtime beyond this slice。

Next action:
- `queued -> inprogress` を記録してから worktree-workflow で `/home/hare/Projects/yoi/.worktree/00001KVXK0WD3-remove-legacy-wasm-runtime` を作成し、multi-agent-workflow で Coder/Reviewer sibling loop に進める。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-24T20:14:08Z from: queued to: inprogress reason: orchestrator_acceptance_unblocked field: state -->

## State changed

Orchestrator acceptance: queued -> inprogress

- 直前確認で `TicketShow` は state `queued`、blocking outgoing relation は 0 件、accepted plan `orch-plan-20260624-201247-1` を確認した。
- 後続 queued Tickets (`00001KVXK0WDH`, `00001KVXK0WDQ`, `00001KVXK0WDX`, `00001KVXK0WE4`, `00001KVXK0WEA`) は dependency chain 上この Ticket またはその後続 prerequisites に blocked として orchestration plan に記録済み。
- routing decision と IntentPacket は Ticket thread に記録済み。
- これ以降に worktree-workflow で `/home/hare/Projects/yoi/.worktree/00001KVXK0WD3-remove-legacy-wasm-runtime` を作成し、multi-agent-workflow に接続する。

---
