<!-- event: create author: "yoi ticket" at: 2026-07-02T13:54:52Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: plan author: hare at: 2026-07-02T13:56:05Z -->

## Plan

Workspace Backend Runtime connection management と永続 config の Ticket として詳細化した。

方針:
- Runtime が Backend に接続するのではなく、Backend が Runtime source を RuntimeRegistry に登録する現行構造を前提にする。
- embedded Runtime は built-in connection として表示し、config 管理対象や削除対象にしない。
- remote Runtime connection は `.yoi/workspace-backend.local.toml` の `[[runtimes.remote]]` に保存する。
- 管理 API は list/add/delete/test を提供する。
- v0 は config persistence を優先し、config 更新後は `restart_required = true` とする。live register/unregister は対象外。
- raw token 値、socket/session/store path、Runtime event cursor、live handle は UI/API/config に出さない。


---

<!-- event: decision author: hare at: 2026-07-02T14:00:49Z -->

## Decision

Runtime connection management Ticket は Settings shell 先行 Ticket に依存する形へ整理した。

Decision:
- `00001KWHJ0XH6` が Workspace-local Settings の entry point / route / shell / section navigation を用意する。
- `00001KWHHRTM9` はその Settings shell 上に Runtime Connections section と Backend API / config persistence を追加する。
- Runtime connection 管理 Ticket 内で Settings shell 設計を同時に進めない。


---

<!-- event: decision author: hare at: 2026-07-02T14:55:21Z -->

## Decision

Runtime connection management に negotiation / compatibility check を含める方針へ更新した。

Decision:
- 現状は明示的な negotiation / handshake は無く、`GET /v1/runtime` が事実上の疎通確認になっているだけ。
- Runtime connection `test` は単なる ping ではなく lightweight negotiation / compatibility check として扱う。
- v0 check は `GET /v1/runtime` の parse、observed runtime id/display/status/capabilities、worker list/detail、event websocket construction、spawn/input/config-bundle support などを確認する。
- 現行 API に protocol version が無い場合は fake version を作らず、`v1 runtime endpoint responded` のような compatibility basis として表現する。
- negotiation result / observed capabilities / health result / checked_at / diagnostics は persisted config に保存しない。必要になった場合は Backend DB の observation/cache として扱う。
- persisted config、RuntimeRegistry live state、negotiation result を混同しない。


---

<!-- event: intake_summary author: hare at: 2026-07-02T16:25:43Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-02T16:25:43Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-07-02T16:45:19Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-07-02T16:59:57Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Ticket は Runtime connection settings API / `.yoi/workspace-backend.local.toml` 永続化 / Settings UI / sanitized diagnostics / restart_required の v0 境界を具体化しており、observable な acceptance criteria と validation がある。
- 先行 Settings shell Ticket `00001KWHJ0XH6` は closed を確認済み。
- typed relation blocker は 0 件、OrchestrationPlan は human gate の waiting note のみで、ユーザーから「2つとも消化して」と明示 follow-up があったため human gate は解除された。
- Manual Coding Worker Ticket `00001KWHEM8YJ` と UI/API surface が近く、並列別 worktree では衝突リスクが高いので、同一 implementation worktree/branch で Runtime connection 管理を先に実装し、続けて manual Worker 作成導線を実装する。

Evidence checked:
- Ticket body / thread / artifacts。
- `TicketRelationQuery(00001KWHHRTM9)` は 0 件。
- `TicketOrchestrationPlanQuery(00001KWHHRTM9)` は prior human-gate waiting note のみ。
- `TicketShow(00001KWHJ0XH6)` は closed。
- queued Ticket 一覧では `00001KWHEM8YJ` も queued、ready/inprogress は 0 件。
- workspace/orchestration git state と worktree 一覧、visible Pods、TicketDoctor（0 errors / 既存 warning のみ）。
- Bounded code map: `crates/workspace-server` の workspace backend / runtime registry / config file 周辺、`web/workspace/src/lib/workspace-settings/SettingsPage.svelte`、`web/workspace/src/routes/settings/+page.svelte`、既存 `/api/workers` / `/api/runtimes/{runtime_id}/workers` projection、`WorkersNavSection.svelte`。

IntentPacket:

Intent:
- Workspace Browser の Settings / Runtime Connections で embedded Runtime 表示、remote Runtime connection の list/add/delete/test negotiation、`.yoi/workspace-backend.local.toml` 永続化、restart_required diagnostic を提供する。
- `00001KWHEM8YJ` が使う Runtime candidate projection の基盤を作る。

Binding decisions / invariants:
- Runtime が Backend に接続するのではなく、Backend が configured Runtime source を `RuntimeRegistry` に登録する現行構造を前提にする。
- embedded Runtime は built-in として表示し、config 管理対象や削除対象にしない。
- remote Runtime connection は既存 `[[runtimes.remote]]` schema に保存する。
- v0 は config persistence 優先で、config 更新後は `restart_required = true`。live register/unregister は対象外。
- raw token 値、secret、socket/session/store path、Runtime event cursor、live handle、config file path は UI/API response に出さない。
- negotiation/test result、observed capabilities、health result、checked_at は local config に保存しない。
- protocol version が無ければ fake version を作らず、compatibility basis として表現する。

Requirements / acceptance criteria:
- Runtime Connections 管理画面が Settings shell 内にある。
- embedded Runtime は built-in / delete不可として表示される。
- remote connection を add/delete でき、config の `[[runtimes.remote]]` が read-modify-write される。
- test negotiation は `GET /v1/runtime` parse と Browser 必要操作に対する compatibility/sanitized diagnostics を返す。
- duplicate id / invalid endpoint / incompatible runtime / embedded delete attempt は typed diagnostic。
- request/response/config に raw token 値や internal paths を含めない。
- focused backend/UI tests を追加し、指定 validation を可能な範囲で実行する。

Implementation latitude:
- TOML comments/format preservation は v0 で typed serialize により失われてもよい。ただし implementation report/docs/test で明記する。
- POST 保存前に test を必須にするか、保存は許して test diagnostic を別扱いにするかは、Ticket の v0 境界内で選んでよい。
- API response shape / Svelte component分割 / test helper構成は既存 style に合わせて選んでよい。

Escalate if:
- secret store / raw token input / live RuntimeRegistry unregister / remote workspace provisioning / protocol version追加を必須にしないと満たせない場合。
- existing config schema を壊す必要がある場合。
- Browser-facing API に raw path/secret/runtime internal location を出す誘惑が出た場合。

Validation:
- `cd web/workspace && deno task test`
- `cd web/workspace && deno task check`
- `cargo test -p yoi-workspace-server`
- `cargo check -p yoi`
- `git diff --check`
- `nix build .#yoi --no-link` は時間/依存変更の重さを見て実行、未実行なら理由を report。

Current code map:
- Settings UI: `web/workspace/src/lib/workspace-settings/SettingsPage.svelte`, `web/workspace/src/routes/settings/+page.svelte`。
- Worker/sidebar UI: `web/workspace/src/lib/workspace-sidebar/WorkersNavSection.svelte`, console route under `web/workspace/src/routes/runtimes/[runtimeId]/workers/[workerId]/console/`。
- Backend/runtime/config areas: `crates/workspace-server`, existing `/api/workers`, `/api/runtimes/{runtime_id}/workers`, `WorkspaceBackendConfigFile`, `ServerConfig.remote_runtime_sources`, `RuntimeRegistry`, `EmbeddedWorkerRuntime`。

Critical risks / reviewer focus:
- persisted config、live registry state、test/negotiation observation を混同していないか。
- Browser-facing API に secrets/internal paths/runtime store/socket/session/config file path が漏れていないか。
- embedded delete が fail closed か。
- restart_required semantics が UI/API で明確か。
- `00001KWHEM8YJ` と同一 worktreeで実装し、runtime candidates の shared contract が破綻していないか。

---

<!-- event: state_changed author: orchestrator at: 2026-07-02T17:01:01Z from: queued to: inprogress reason: routing_acceptance_implementation_ready field: state -->

## State changed

Queued acceptance recorded after explicit user follow-up 「2つとも消化して」。

Checked context:
- Ticket body / thread / artifacts。
- `TicketRelationQuery(00001KWHHRTM9)`: blocking relation 0 件。
- `TicketOrchestrationPlanQuery(00001KWHHRTM9)`: prior human-gate waiting note を確認し、今回 accepted_plan / before ordering を記録済み。
- `00001KWHJ0XH6` は closed。
- workspace/worktree/visible Pod/TicketDoctor/code-map の bounded check。

Acceptance basis:
- concrete missing decision / information は残っていない。
- `00001KWHEM8YJ` と surface が重なるため、同一 implementation worktree でこの Ticket を先に実装する。
- side effect はこの `queued -> inprogress` acceptance 後に開始する。

---

<!-- event: implementation_report author: orchestrator at: 2026-07-02T17:02:35Z -->

## Implementation report

Implementation side-effect recovery note:

- `queued -> inprogress` acceptance 後、implementation worktree `/home/hare/Projects/yoi/.worktree/00001KWHHRTM9-00001KWHEM8YJ` / branch `work/00001KWHHRTM9-00001KWHEM8YJ` は作成済み。
- 最初の Coder Pod spawn は、child workspace root `/home/hare/Projects/yoi` を readable scope に含めていなかったため `workspace root is not readable under the configured scope` で失敗した。
- Ticket は `inprogress` のまま、read scope に workspace root、write scope に implementation worktree だけを含める形で即時 retry する。raw runtime/socket/session details は implementation contract には含めない。

---

<!-- event: implementation_report author: orchestrator at: 2026-07-02T17:03:28Z -->

## Implementation report

Implementation routing update:

- Recovery retry succeeded: sibling Coder Pod `yoi-coder-00001KWHHRTM9-00001KWHEM8YJ` was spawned for the combined branch/worktree.
- Scope policy: workspace root is readable for runtime workspace metadata; write authority is restricted to implementation worktree `/home/hare/Projects/yoi/.worktree/00001KWHHRTM9-00001KWHEM8YJ`.
- Work order remains: this Ticket first, then `00001KWHEM8YJ`; external review will be requested after coder completion evidence is available.

---

<!-- event: implementation_report author: orchestrator at: 2026-07-02T17:29:34Z -->

## Implementation report

Implementation progress report:

- Coder Pod completed the combined implementation and committed `f2fead7e feat: add workspace runtime and worker controls` on branch `work/00001KWHHRTM9-00001KWHEM8YJ`.
- Worktree status was clean after commit.
- Changed areas include `crates/workspace-server/src/config.rs`, `crates/workspace-server/src/hosts.rs`, `crates/workspace-server/src/server.rs`, Settings UI/model/tests, Worker sidebar UI/types, and workspace TS config/CSS.
- Orchestrator reran validation:
  - `git diff --check`: pass
  - `cargo test -p yoi-workspace-server`: pass（51 lib tests + 2 main tests）
  - `cargo check -p yoi`: pass
  - `cd web/workspace && deno task test`: pass（11 tests）
  - `cd web/workspace && deno task check`: pass（0 errors / 0 warnings）
- `nix build .#yoi --no-link` は dependency/resource packaging 変更ではないため未実行。
- External review requested via sibling Reviewer Pod `yoi-reviewer-00001KWHHRTM9-00001KWHEM8YJ`.

---

<!-- event: review author: reviewer at: 2026-07-02T17:36:45Z status: request_changes -->

## Review: request changes

External review result: request_changes

Checked evidence:
- 両 Ticket record と最新 IntentPacket / orchestration-plan を確認。
- `f2fead7e` の diff/stat/check evidence を確認（10 files changed、`git diff --check f2fead7e^ f2fead7e` clean）。
- `config.rs`, `hosts.rs`, `server.rs`, settings UI/model/tests, sidebar worker form/types, CSS/tsconfig を focused static review。
- Reviewer は read-only で、validation 再実行はせず Orchestrator reported validation pass を参照。

Blockers:

1. Runtime connection `test` が recorded lightweight negotiation / compatibility contract を満たしていない。
   - Ticket は observed runtime capabilities の parse と Browser 必要 operation（list workers, observe detail, event websocket construction, spawn, input dispatch, config-bundle availability/sync）に対する compatibility check を要求している。
   - 現状 `test_remote_runtime_config` は `/v1/runtime` を呼び、狭い `RuntimeHttpSummaryResponse` を parse した後、operation compatibility check なしで常に `state: "compatible"`, `capabilities: Vec::new()` を返す。
   - acceptance criterion の sanitized compatibility/capabilities/diagnostics response を満たしていない。

2. Browser-facing diagnostic sanitization が endpoint/path leak invariant に対して不十分。
   - remote test failure path が `sanitize_backend_error(&error.to_string())` を Browser response に入れている。
   - sanitizer は `/home/`, `/Users/`, `\\`, `:\\` を含む message だけ置換するため、reqwest error の requested URL や non-home config/temp path が漏れる可能性がある。
   - Ticket は Runtime endpoint/config path/internal location を Browser-facing response に漏らさないことを要求しているため、raw backend error string ではなく fixed typed diagnostics にする必要がある。

3. Manual Worker launch が rejected Runtime operation の typed unsupported diagnostics を落としている。
   - `create_workspace_worker` が non-`Accepted` `WorkerOperationResult` を generic `workspace_worker_create_failed` に変換し、`result.diagnostics` を破棄している。
   - unsupported remote/runtime cases は typed sanitized diagnostics を返す必要がある。

4. key contracts の focused acceptance tests が不足。
   - backend は helper-level validation/sanitizer tests に寄っており、runtime connection list/add/delete/test negotiation、config persistence、restart_required、`/api/workers` POST success/unsupported diagnostics の route-level tests が足りない。
   - Web は settings model helper tests のみで、New Worker form rendering/submit-path test がない。

Non-blocking follow-ups:
- manual Worker launch の `display_name` は optional と記録されているが backend/UI は non-empty を要求している。空なら default label derivation を検討。
- Browser-facing request structs に `deny_unknown_fields` を付けると “only these fields” boundary が fail closed になる。
- `nix build .#yoi --no-link` は dependency/resource packaging 変更ではないため未実行だが、acceptance evidence としては未検証。

---
