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

<!-- event: state_changed author: workspace-panel at: 2026-06-24T20:12:03Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-25T06:22:18Z -->

## Decision

Routing decision: implementation_ready

Reason:
- ユーザーから「続けて」と明示 follow-up があり、queued dependency chain の次 Ticket として再確認した。
- `00001KVXK0WE4` は host-owned WebSocket driver の concrete slice で、Discord protocol、full reconnect/resume policy、secret store/auth injection、browser-facing WebSocket API、WIT/PDK templates update は non-goal として分離されている。
- outgoing `depends_on` は `00001KVXK0WDQ` と `00001KVXK0WDX` だが、両方とも done / merged / reviewed / validated 済み。`TicketShow` derived blockers は空で、implementation acceptance blocker は残っていない。
- incoming dependent `00001KVXK0WEA` は WIT/PDK/templates update で、この Ticket 完了後に進めるべき後続であり、この Ticket の acceptance blocker ではない。
- bounded context check で current `crates/pod/src/feature/plugin.rs` に existing pull `host_api.websocket` helpers、WebSocket grants/allowlist、Plugin Service lifecycle / ingress queue、Service output command model が存在することを確認した。Ticket の残る不確実性は host-owned connection driver / frame-to-ingress / output command send integration の local implementation に閉じる。

Evidence checked:
- Ticket body / thread: `item.md`, `thread.md`。未解決 planning question は記録されていない。
- Relations / orchestration plan: outgoing depends_on `00001KVXK0WDQ` と `00001KVXK0WDX` は done。routing 前 plan は historical blocked_by records 2 件で、prerequisites 完了により解消済み。accepted plan `orch-plan-20260625-062148-3` を記録済み。
- Related Tickets: `00001KVXK0WDQ` and `00001KVXK0WDX` are done.
- Code context: `crates/pod/src/feature/plugin.rs` の WebSocket allowlist/grants, existing pull API helpers, Plugin Service ingress queue, output command model。
- Workspace state: `/home/hare/Projects/yoi/.worktree/orchestration` は clean。inprogress Ticket は 0 件。

IntentPacket:

Intent:
- Service Plugin に紐づく host-owned WebSocket connection driver を追加し、incoming text frames を Plugin ingress queue に event として enqueue し、Service output command の `websocket_send` を grant-check 後に Host が送信できるようにする。

Binding decisions / invariants:
- Connection ownership is Host-side. Plugin code does not run a long-lived recv loop as recommended service path.
- Existing pull `host_api.websocket.recv(timeout)` API may remain for compatibility/internal bounded use, but docs/recommended service integration path must move away from it.
- WebSocket target authority is manifest declaration + enabled grant / allowlist. Unauthorized target/send fails closed.
- v0 handles text frames. Binary application payload is unsupported/diagnostic. ping/pong/close handling should be bounded/control/diagnostic, not app payload.
- Incoming frames are converted to service ingress events using existing bounded queue and lifecycle diagnostics.
- Outgoing sends use Service output command path from `00001KVXK0WDX`; do not bypass its validation/grant boundary.
- Full reconnect/resume policy, Discord/Slack protocol logic, secret injection design, browser-facing WebSocket API, WIT/PDK/templates update are non-goals.

Requirements / acceptance criteria:
- Host-owned WebSocket driver can manage a connection associated with a Service Plugin.
- Incoming text frames enqueue Plugin ingress events.
- Close/error/reconnect-needed states become ingress events or status diagnostics.
- Plugin output command can trigger WebSocket text send after grant checks.
- Ungranted send / unauthorized target is fail-closed diagnostic.
- Connection status, last frame time, last error, queue drops, send failures are visible in diagnostics/status.
- Tests cover driver lifecycle, incoming frame to ingress, send command execution/rejection, close/error diagnostics, and frame kind handling.

Implementation latitude:
- Exact struct names and whether tests use mock WebSocket client/connection or local bounded fake are coder choices.
- v0 may model reconnect-needed as diagnostic instead of automatic reconnect.
- Keep integration in `crates/pod/src/feature/plugin.rs` unless a small helper module split is clearly cleaner.

Escalate if:
- Real network tests or external services are required.
- Secret store/auth injection becomes necessary.
- Durable cross-process queue or scheduler semantics are required.
- Implementing this requires WIT/PDK/template changes rather than Rust-side host runtime only.
- Browser-facing WebSocket API or protocol-specific Discord/Slack behavior is needed.

Validation:
- `cargo test -p pod`
- `cargo check -p yoi`
- `git diff --check`
- `nix build .#yoi --no-link`
- Focused WebSocket driver / service ingress tests during development.

Current code map:
- Primary: `crates/pod/src/feature/plugin.rs`。
- Secondary only if necessary: docs/development recommended path wording。
- Avoid: WIT/PDK/templates update (reserved for `00001KVXK0WEA`), protocol-specific integrations, full reconnect policy, secret store, browser WebSocket API。

Critical risks / reviewer focus:
- hidden ambient network authority or bypassing manifest/grant allowlist。
- pull `recv(timeout)` remaining as recommended long-lived service path。
- output command send bypassing Service command validation。
- unbounded reader task/queue or dropped diagnostics。
- tests that require real external network。
- scope creep into PDK/templates or protocol-specific behavior。

Next action:
- `queued -> inprogress` を記録してから worktree-workflow で `/home/hare/Projects/yoi/.worktree/00001KVXK0WE4-plugin-websocket-driver` を作成し、multi-agent-workflow で Coder/Reviewer sibling loop に進める。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-25T06:23:16Z from: queued to: inprogress reason: orchestrator_acceptance_unblocked field: state -->

## State changed

Orchestrator acceptance: queued -> inprogress

- 直前確認で `TicketShow` は state `queued`、derived blockers は空。
- outgoing dependencies `00001KVXK0WDQ` と `00001KVXK0WDX` は done / merged / reviewed / validated 済み。
- accepted plan `orch-plan-20260625-062148-3` を確認した。
- routing decision と IntentPacket は Ticket thread に記録済み。
- これ以降に worktree-workflow で `/home/hare/Projects/yoi/.worktree/00001KVXK0WE4-plugin-websocket-driver` を作成し、multi-agent-workflow に接続する。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-25T06:24:16Z -->

## Implementation report

Implementation orchestration update:

- prerequisite `00001KVXK0WDX` が done になったため、`00001KVXK0WE4` を再 routing して受理した。
- worktree-workflow に従い、implementation worktree `/home/hare/Projects/yoi/.worktree/00001KVXK0WE4-plugin-websocket-driver` と branch `work/00001KVXK0WE4-plugin-websocket-driver` を作成した。
- child worktree では tracked `.yoi` project records を見える状態にし、`.yoi/memory`、local/runtime/log/lock/secret-like `.yoi` paths を sparse-checkout で除外する方針にした。
- Coder Pod `yoi-coder-00001KVXK0WE4-websocket-driver` を sibling として起動した。write scope は child worktree に限定し、runtime workspace root `/home/hare/Projects/yoi` は startup 要件を満たすため non-recursive read のみにした。
- Coder には root/original workspace と orchestration worktree で read/write/validation/git operation を行わないよう明示した。

---

<!-- event: implementation_report author: yoi-coder-00001KVXK0WE4-websocket-driver at: 2026-06-25T06:56:37Z -->

## Implementation report

Implementation report:

- Ticket: `00001KVXK0WE4`
- Branch/worktree: `work/00001KVXK0WE4-plugin-websocket-driver` / `/home/hare/Projects/yoi/.worktree/00001KVXK0WE4-plugin-websocket-driver`
- Commit: `f2c51ffe399fc4eaa0ab4e4052867d55e534b229`
- Commit message: `feat: add plugin websocket service driver`

変更ファイル:
- `crates/pod/src/feature/plugin.rs`
  - Service Plugin 用の host-owned WebSocket driver を追加。
  - `ingress.sources` の `websocket:wss://...` / `ws(s)://...` を Service WebSocket 接続として扱う実装を追加。
  - incoming text frame を既存の bounded Service ingress queue へ `websocket_text` event として enqueue。
  - close/error を `websocket_close` / `websocket_error` ingress event と status diagnostic に反映。
  - `websocket_send` Service output command を host-owned connection 経由の text send に接続。
  - manifest declaration + enabled grant / allowlist を通らない open/send は fail-closed。
  - status に connection state、last frame time、last error、received/sent count、queue drops、send failures を追加。
  - driver lifecycle / incoming / send success / send rejection / send failure / close-error diagnostics / binary-like frame error の focused tests を追加。
- `docs/development/plugin-development.md`
  - Service Plugin の推奨 WebSocket integration を、guest long-lived recv loop ではなく host-owned Service WebSocket driver + `websocket_send` output command に更新。

検証結果:
- `cargo test -p pod --lib websocket_`: success (`14 passed`)
- `cargo test -p pod --lib service_output_command`: success (`4 passed`)
- `cargo check -p pod`: success
- `cargo check -p yoi`: success
- `git diff --check`: success
- `nix build .#yoi --no-link`: success
  - Nix dirty warning は worktree の ignored `target/` によるもので、tracked changes はなし。
- `cargo test -p pod`: failed only in known prompt guidance snapshot tests outside this Ticket (`438 passed; 2 failed`)
  - `prompt::catalog::tests::pod_orchestration_guidance_section_renders_resource_body`
  - `prompt::system::tests::pod_orchestration_guidance_is_included_for_pod_management_tools`

Residual risks / notes:
- v0 は自動 reconnect/resume policy を実装せず、close/error は diagnostic/event 化に留めている。
- Service WebSocket source の manifest 表現は既存 `ingress.sources` の文字列規約として追加。WIT/PDK/templates 更新は Ticket `00001KVXK0WEA` 側に残している。
- 実ネットワーク test は non-goal のため、mock WebSocket client/connection による bounded focused tests で検証している。

---

<!-- event: review author: yoi-reviewer-00001KVXK0WE4-websocket-driver at: 2026-06-25T07:03:47Z status: approve -->

## Review: approve

Review result: approve

読み取り専用で Ticket / IntentPacket / `62e467c0..HEAD` diff を確認した。Ticket review 記録・merge・close・検証コマンド実行は reviewer では行っていない。

確認した証拠:
- 変更範囲は `crates/pod/src/feature/plugin.rs` と `docs/development/plugin-development.md` のみで、WIT / PDK / templates へのスコープ拡大はない。
- Host-owned Service WebSocket driver:
  - Service/Ingress 有効時に `ingress.sources` から `websocket:` / `ws(s)://` source を subscription 化している。
  - instance start 後に host 側で connection start し、reader thread を生成している。
- authority / fail-closed:
  - open は `host_api.websocket` permission と manifest/grant allowlist を通した既存 validation を再利用している。
  - `websocket_send` は Service output command envelope の validation/grant check 経由でのみ実行され、URL scheme/credentials/static target/manifest+grant allowlist を検査している。
- ingress queue / diagnostics:
  - text frame は `websocket_text` event として既存 `deliver_ingress` path に渡され、失敗時は queue drop と diagnostic を記録する。
  - close/error は `websocket_close` / `websocket_error` event と status diagnostic に反映される。
  - status には connection state, last frame, last error, received/sent counts, queue drops, send failures が出る。
- output command send:
  - `PluginServiceOutputCommandKind::WebSocketSend` が追加され、実行時は host-owned connection の `send_text` に接続され、send failure diagnostic も記録する。
- docs:
  - `recv(timeout)` は bounded/internal/tool-style use として残しつつ、Service Plugin では host-owned Service WebSocket driver + `websocket_send` を推奨する文面に更新されている。
- tests:
  - mock/fake client による incoming text/close、send success/failure、unauthorized target、receive error/binary相当 diagnostic のテストが追加されている。

検証状況:
- reviewer は cargo/nix 等を実行していない（read-only / no writes 指示のため）。
- diff・対象コード・Ticket acceptance を静的確認し、coder 報告の `cargo check` / focused tests / nix build 結果を前提証跡として扱った。

Non-blocking notes:
- `docs/development/plugin-development.md` の “Current status” 付近にはまだ Service/Ingress/WebSocket を future work と読む余地が残っているが、本 Ticket の受け入れ条件である「pull recv を recommended service path にしない」は満たしている。後続の docs/PDK/templates Ticket で整理するとよい。
- close/error event の enqueue 失敗は text frame のような queue-drop counter には載せていないが、status diagnostic は記録されるため今回の acceptance 上は blocker ではない。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-25T07:06:19Z -->

## Implementation report

Integration outcome:

- Coder `yoi-coder-00001KVXK0WE4-websocket-driver` が implementation branch `work/00001KVXK0WE4-plugin-websocket-driver` に実装を commit した。
  - implementation commit: `f2c51ffe feat: add plugin websocket service driver`
- Reviewer `yoi-reviewer-00001KVXK0WE4-websocket-driver` は read-only review で `approve`。host-owned connection authority、manifest/grant allowlist、ingress queue integration、`websocket_send` command validation、status diagnostics、scope creep なしを確認した。
- Orchestrator worktree `/home/hare/Projects/yoi/.worktree/orchestration` で branch `work/00001KVXK0WE4-plugin-websocket-driver` を merge 済み。
  - merge commit: `b9e786e1 merge: 00001KVXK0WE4 plugin websocket driver`

Implemented behavior:
- `crates/pod/src/feature/plugin.rs` に Service Plugin 用 host-owned WebSocket driver を追加。
- `ingress.sources` の `websocket:wss://...` / `ws(s)://...` source を Service WebSocket connection として扱う。
- incoming text frame を existing bounded Service ingress queue へ `websocket_text` event として enqueue。
- close/error は `websocket_close` / `websocket_error` event と status diagnostic に反映。
- `websocket_send` Service output command は host-owned connection 経由の text send に接続。
- manifest declaration + enabled grant / allowlist を通らない open/send は fail-closed。
- status に connection state, last frame time, last error, received/sent counts, queue drops, send failures を追加。
- `docs/development/plugin-development.md` の Service Plugin 推奨 WebSocket integration を host-owned driver + `websocket_send` output command に更新。

Validation in Orchestrator worktree:
- `cargo test -p pod --lib websocket_`: success
- `cargo test -p pod --lib service_output_command`: success
- `cargo check -p pod`: success
- `cargo check -p yoi`: success
- `git diff --check`: success
- `nix build .#yoi --no-link`: success
- `cargo test -p pod`: failed only in known prompt guidance snapshot assertions outside this Plugin diff:
  - `prompt::catalog::tests::pod_orchestration_guidance_section_renders_resource_body`
  - `prompt::system::tests::pod_orchestration_guidance_is_included_for_pod_management_tools`
  - Plugin/WebSocket focused tests in the same run passed.

Notes:
- Reviewer non-blocking note: `docs/development/plugin-development.md` の “Current status” 付近にはまだ Service/Ingress/WebSocket を future work と読む余地が残る。後続 `00001KVXK0WEA` の docs/PDK/templates update で整理するのがよい。
- Reviewer non-blocking note: close/error event enqueue failure は text frame と同じ queue-drop counter には載らないが、status diagnostic は記録されるため blocker ではない。
- Follow-up `00001KVXK0WEA` is dependency-unblocked and can be re-routed next.

Next action:
- Mark Ticket done after this integration/validation evidence.
- Then stop related child Pods and remove only the child implementation worktree/branch.

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-25T07:06:30Z from: inprogress to: done reason: merged_validated_with_unrelated_full_suite_caveat field: state -->

## State changed

Implementation is merged into the orchestration branch and validated with one unrelated known full-suite caveat.

Evidence:
- merge commit: `b9e786e1 merge: 00001KVXK0WE4 plugin websocket driver`
- reviewer result: approve
- validation in `/home/hare/Projects/yoi/.worktree/orchestration`:
  - `cargo test -p pod --lib websocket_`: success
  - `cargo test -p pod --lib service_output_command`: success
  - `cargo check -p pod`: success
  - `cargo check -p yoi`: success
  - `git diff --check`: success
  - `nix build .#yoi --no-link`: success
  - `cargo test -p pod`: failed only in prompt guidance snapshot assertions outside this Plugin diff; Plugin/WebSocket-focused tests passed.

Closure is not performed here; this state records implementation completion after merge/review/focused validation.

---

<!-- event: state_changed author: hare at: 2026-06-25T14:13:52Z from: done to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-06-25T14:13:52Z status: closed -->

## 完了

Implemented, reviewed, marked done, and merged into develop.


---
