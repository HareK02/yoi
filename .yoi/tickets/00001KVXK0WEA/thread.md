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

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-25T07:08:46Z from: queued to: inprogress reason: orchestrator_acceptance_unblocked field: state -->

## State changed

Orchestrator acceptance: queued -> inprogress

- 直前確認で `TicketShow` は state `queued`、derived blockers は空。
- outgoing dependency `00001KVXK0WE4` は done / merged / reviewed / validated 済み。
- accepted plan `orch-plan-20260625-070739-2` を確認した。
- routing decision と IntentPacket は Ticket thread に記録済み。
- これ以降に worktree-workflow で `/home/hare/Projects/yoi/.worktree/00001KVXK0WEA-plugin-pdk-service-events` を作成し、multi-agent-workflow に接続する。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-25T07:09:39Z -->

## Implementation report

Implementation orchestration update:

- prerequisite `00001KVXK0WE4` が done になったため、`00001KVXK0WEA` を再 routing して受理した。
- worktree-workflow に従い、implementation worktree `/home/hare/Projects/yoi/.worktree/00001KVXK0WEA-plugin-pdk-service-events` と branch `work/00001KVXK0WEA-plugin-pdk-service-events` を作成した。
- child worktree では tracked `.yoi` project records を見える状態にし、`.yoi/memory`、local/runtime/log/lock/secret-like `.yoi` paths を sparse-checkout で除外する方針にした。
- Coder Pod `yoi-coder-00001KVXK0WEA-pdk-service-events` を sibling として起動した。write scope は child worktree に限定し、runtime workspace root `/home/hare/Projects/yoi` は startup 要件を満たすため non-recursive read のみにした。
- Coder には root/original workspace と orchestration worktree で read/write/validation/git operation を行わないよう明示した。

---

<!-- event: implementation_report author: yoi-coder-00001KVXK0WEA-pdk-service-events at: 2026-06-25T07:38:44Z -->

## Implementation report

Implementation report:

対象 Ticket:
- `00001KVXK0WEA` — Update Plugin WIT PDK templates for service event runtime

実装概要:
- Plugin authoring surface を Component Model-only / Service ingress event / output command model に合わせて更新した。

変更ファイル:
- `resources/plugin/wit/yoi-plugin-instance-v1.wit`
  - Service ingress event JSON と `output_commands` / `websocket_send` の authoring 向け説明を追加。
- `resources/plugin/wit/yoi-plugin-tool-v1.wit`
  - current runtime 前提の説明に整理。
- `crates/plugin-pdk/src/lib.rs`
  - `PluginIngressEvent` に `ingress_name`, `created_at`, `attempt`, `correlation_id` を追加。
  - `ServiceOutput`, `ServiceOutputCommand`, `ServiceOutputCommandKind` を追加。
  - `PluginIngressEvent::websocket_text()`, `websocket_send()`, `ServiceOutput::websocket_send()` を追加。
  - `Plugin::handle_ingress` の戻り値を `ServiceOutput` に変更。
- `crates/plugin-pdk/tests/template.rs`
  - PDK helper / WIT / template consistency tests を追加・更新。
- `resources/plugin/templates/rust-component-instance/*`
  - Service-oriented template に更新。
  - guest-side `recv(timeout)` loop ではなく、`handle_ingress` で ingress event を受け、`ServiceOutput::websocket_send` で output command を返す構成に変更。
  - `plugin.toml` は `runtime.kind = "wasm-component"` のみ。
  - Tool handler も残し、Tool Plugin authoring support を維持。
- `crates/yoi/src/plugin_cli.rs`, `crates/yoi/src/main.rs`
  - `yoi plugin new rust-component-service <path>` を追加。
  - service template の `new` / `check` / `pack` consistency を unit test で確認。
  - CLI usage を更新。
- `docs/development/plugin-development.md`
  - Service/Ingress は host-owned ingress event + output command model として記述。
  - Service WebSocket integration で long-running guest `recv(timeout)` loop を推奨しない内容に更新。

Validation:
- `cargo test -p yoi-plugin-pdk`: success
- `cargo test -p yoi plugin`: success
- `cargo check -p yoi`: success
- `git diff --check`: success
- `nix build .#yoi --no-link`: success (`nix-exit:0`)
- Template cargo-check: success
  - `cargo test -p yoi-plugin-pdk` 内で `rust-component-tool` / `rust-component-instance` の `cargo check --target wasm32-unknown-unknown --offline --target-dir <temp>` を実行し、temporary target dir を使用。

Commit:
- `7a4fd975268b655dca2f0861b8f2bd767c1e914f feat: update plugin service authoring templates`

Residual risks / notes:
- Runtime envelope の再設計は行っていない。
- protocol-specific integration / secret injection / reconnect policy / runtime reimplementation は範囲外として未実装。

---

<!-- event: review author: yoi-reviewer-00001KVXK0WEA-pdk-service-events at: 2026-06-25T07:44:48Z status: request_changes -->

## Review: request changes

Review result: request_changes

実装は概ね Ticket の方向に沿っているが、Service WebSocket template の manifest が runtime の authority boundary と不整合で、生成テンプレートの中心例が実行時に `websocket_send` できない状態に見える。

Blocker:
- Service template が `websocket_send` に必要な manifest 権限/target を宣言していない。
  - `resources/plugin/templates/rust-component-instance/src/lib.rs` は `ServiceOutput::websocket_send(...)` を返す Service/WebSocket 例になっている。
  - `resources/plugin/templates/rust-component-instance/plugin.toml` は `surface/tool/service/ingress` と `tool/service/ingress` permission はあるが、`{ kind = "host_api", api = "websocket" }` がない。
  - 同 `plugin.toml` には matching `[[websocket]]` target declaration もない。
  - runtime 側の `websocket_send` 処理は manifest permissions の `host_api.websocket`、grants permissions の `host_api.websocket`、manifest の `[[websocket]]` target declaration、enablement 側の usable websocket grant を要求する。
  - そのため生成された service template は `yoi plugin check/pack` は通っても、実行時に `requested host_api.websocket permission is missing` / `manifest host_api.websocket target declaration is missing` 系で拒否されるはず。

Required fixes:
- `resources/plugin/templates/rust-component-instance/plugin.toml` に少なくとも以下を追加する。
  - `permissions` の `{ kind = "host_api", api = "websocket" }`
  - `[[websocket]]` target declaration（例: `scheme = "wss"`, `host = "example.com"`, `path_prefixes = ["/socket"]`）
- template/CLI tests に、service template が `host_api.websocket` permission と matching `[[websocket]]` declaration を含むことの assertion を追加する。
- 必要なら README/docs で、enablement grants も同じ WebSocket target を明示的に許可する必要があることを補足する。

Good points:
- WIT は `handle-ingress`、ingress event envelope、`output_commands` / `websocket_send` 例を記述している。
- PDK は `PluginIngressEvent`、`ServiceOutput`、`ServiceOutputCommand`、`websocket_text()` / `websocket_send()` helper を追加し、runtime envelope 形状に概ね合っている。
- Service template は `recv(timeout)` polling loop を推奨しておらず、`handle_ingress` + `ServiceOutput::websocket_send` の形になっている。
- template `runtime.kind` は `wasm-component` で、raw `wasm` template 復活は見当たらない。
- `yoi plugin new rust-component-service` は既存の path-safety 方針上に追加されており、広い CLI redesign や runtime reimplementation は見当たらない。

Validation:
- reviewer は read-only 指示のため cargo/nix/git diff validation は再実行していない。
- Ticket item/thread、IntentPacket、`cd200630..HEAD` の変更、WIT / PDK / template / CLI / docs / runtime envelope 関連箇所を静的確認した。

---

<!-- event: implementation_report author: yoi-coder-00001KVXK0WEA-pdk-service-events at: 2026-06-25T07:53:08Z -->

## Implementation report

Follow-up implementation report after review request_changes:

対応内容:
- Reviewer blocker に対応し、Service template の `websocket_send` output command に必要な runtime authority を追加した。

変更点:
- `resources/plugin/templates/rust-component-instance/plugin.toml`
  - `permissions` に `{ kind = "host_api", api = "websocket" }` を追加。
  - matching target として `[[websocket]]` を追加。
    - `scheme = "wss"`
    - `host = "example.com"`
    - `path_prefixes = ["/socket"]`
- `crates/plugin-pdk/tests/template.rs`
  - service template manifest に `host_api.websocket` permission が含まれることを検証。
  - `[[websocket]]` target が `wss://example.com/socket` に対応していることを検証。
- `crates/yoi/src/plugin_cli.rs`
  - `yoi plugin new rust-component-service ...` で生成される manifest に `host_api.websocket` permission / `[[websocket]]` declaration が含まれることを CLI test に追加。
- `resources/plugin/templates/rust-component-instance/README.md`
  - manifest 側の `host_api.websocket` / `[[websocket]]` declaration と、enablement grants 側でも同じ WebSocket target を明示許可する必要があることを追記。
- `docs/development/plugin-development.md`
  - Service ingress / output command の minimal manifest example に `host_api.websocket` permission と `[[websocket]]` target を追加。
  - `websocket_send` output command には manifest declaration だけでなく runtime enablement grants も必要であることを明記。

Validation:
- `cargo test -p yoi-plugin-pdk`: success
- `cargo test -p yoi plugin`: success
- `cargo check -p yoi`: success
- `git diff --check`: success
- `nix build .#yoi --no-link`: success (`nix-exit:0`)

Commit:
- `6c8998878dc1d59ad70da3280efd503a9a060ebb fix: declare plugin service websocket authority`

Residual risks / notes:
- なし。runtime envelope の再設計、raw wasm compatibility、protocol-specific integration、secret/reconnect policy、runtime reimplementation は行っていない。

---
