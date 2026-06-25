<!-- event: create author: "yoi ticket" at: 2026-06-24T19:51:56Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-06-24T19:55:30Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-24T19:55:30Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-24T20:12:05Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-25T07:08:09Z -->

## Decision

Routing decision: implementation_ready

Reason:
- ユーザーから「続けて」と明示 follow-up があり、queued dependency chain の最終 Ticket として再確認した。
- `00001KVXK0WEA` は WIT / PDK / templates / docs を、既に実装済みの Component Model-only runtime、Service lifecycle / ingress queue、Service output commands、host-owned WebSocket driver に合わせる finishing slice である。
- outgoing `depends_on` は `00001KVXK0WE4` だが、`00001KVXK0WE4` は done / merged / reviewed / validated 済み。`TicketShow` derived blockers は空で、implementation acceptance blocker は残っていない。
- bounded context check で `resources/plugin/wit/*`, `crates/plugin-pdk`, `resources/plugin/templates/rust-component-tool`, `crates/yoi/src/plugin_cli.rs`, `docs/development/plugin-development.md` 周辺に authoring-facing surfaces があることを確認した。Ticket は runtime 実装ではなく authoring surface alignment に限定されており、残る不確実性は local implementation / fixture update に閉じる。

Evidence checked:
- Ticket body / thread: `item.md`, `thread.md`。未解決 planning question は記録されていない。
- Relations / orchestration plan: outgoing depends_on `00001KVXK0WE4` は done。routing 前 plan は historical blocked_by `00001KVXK0WE4` のみで、prerequisite 完了により解消済み。accepted plan `orch-plan-20260625-070739-2` を記録済み。
- Related Tickets: `00001KVXK0WD3`, `00001KVXK0WDH`, `00001KVXK0WDQ`, `00001KVXK0WDX`, `00001KVXK0WE4` are done.
- Code/docs context: `resources/plugin/wit/*.wit`, `crates/plugin-pdk`, `resources/plugin/templates/rust-component-tool`, `crates/yoi/src/plugin_cli.rs`, `docs/development/plugin-development.md`。
- Workspace state: `/home/hare/Projects/yoi/.worktree/orchestration` は clean。inprogress Ticket は 0 件。

IntentPacket:

Intent:
- Plugin authoring surface (WIT, Rust PDK, embedded templates, docs, `yoi plugin new/check/pack` fixtures) を、Component Model-only runtime と Service ingress event / output command / host-owned WebSocket model に合わせて更新する。

Binding decisions / invariants:
- `plugin.toml` template は `wasm-component` runtime のみを生成する。
- Service/WebSocket authoring pattern は polling `recv(timeout)` loop ではなく、ingress event handler + output command (`websocket_send`) を正とする。
- Tool Plugin authoring support must continue to work.
- Runtime implementation from previous Tickets is not redesigned here; this Ticket updates WIT/PDK/templates/docs/tests to match it.
- No raw core-Wasm compatibility template or legacy runtime alias is introduced.
- No protocol-specific Discord/Slack integration, secret store/auth injection, or full reconnect policy is implemented.

Requirements / acceptance criteria:
- WIT expresses Service ingress event payloads and output command model enough for authoring/tests.
- `yoi-plugin-pdk` exposes ergonomic Tool and Service/Ingress helpers aligned with runtime JSON envelopes.
- Embedded templates include current Tool template and a service-oriented template or equivalent examples using ingress event / output command pattern.
- `yoi plugin new` / `check` / `pack` are consistent with new templates/schema.
- Docs no longer recommend long-running `recv(timeout)` loop for Service WebSocket integration.
- Tests cover PDK helpers, template generation/check/pack, and docs/manifest fixture consistency.

Implementation latitude:
- Exact WIT/interface names and Rust PDK helper APIs may follow existing PDK style, as long as runtime envelopes and docs are consistent.
- If a full new `plugin new` template name is too large, coder may add minimal service template/example plus CLI support needed to satisfy acceptance, but must keep scope bounded.
- Template wasm build/check may use existing test helpers and temporary target dirs.

Escalate if:
- WIT/PDK update requires changing runtime JSON envelope semantics from previous Tickets.
- `yoi plugin new/check/pack` requires broad CLI redesign.
- Real WebSocket network/protocol integration or secret handling is needed.
- Legacy raw-WASM compatibility has to be restored for templates/tests.

Validation:
- `cargo test -p yoi-plugin-pdk`
- `cargo test -p yoi plugin` or focused plugin CLI/template tests
- `cargo check -p yoi`
- `git diff --check`
- `nix build .#yoi --no-link`
- Template cargo-check if applicable, with cleanup of generated template artifacts.

Current code/docs map:
- Primary: `resources/plugin/wit/*.wit`, `crates/plugin-pdk`, `resources/plugin/templates/rust-component-tool`, possible new template under `resources/plugin/templates/`, `crates/yoi/src/plugin_cli.rs`, `docs/development/plugin-development.md`。
- Secondary: manifest tests/fixtures only as needed.
- Avoid: Pod runtime reimplementation, WebSocket driver changes unless minor doc/test alignment, protocol-specific integrations, secret store/auth injection, raw-WASM compatibility。

Critical risks / reviewer focus:
- PDK/template API drift from runtime JSON envelopes。
- Tool template regression while adding Service support。
- reintroducing `recv(timeout)` as recommended Service pattern。
- template generation/check/pack writing outside destination or leaving build artifacts。
- accidental legacy raw-WASM runtime compatibility in examples。

Next action:
- `queued -> inprogress` を記録してから worktree-workflow で `/home/hare/Projects/yoi/.worktree/00001KVXK0WEA-plugin-pdk-service-events` を作成し、multi-agent-workflow で Coder/Reviewer sibling loop に進める。

---
