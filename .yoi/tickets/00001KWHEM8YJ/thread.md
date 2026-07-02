<!-- event: create author: "yoi ticket" at: 2026-07-02T12:59:57Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: plan author: hare at: 2026-07-02T13:00:58Z -->

## Plan

Workspace Browser から手動 Coding Worker を作成する導線として Ticket を詳細化した。

方針:
- Sidebar WORKER heading 横に `New` button を追加する。
- New form は runtime / display name / initial text の最小入力にする。
- Browser は Runtime create request を直接送らず、product-level `/api/workers` endpoint を使う。
- Backend が `kind=coding` を default coding Worker launch に解決する。
- Browser-facing payload に raw path / cwd / ConfigBundleRef / requested capabilities / secret / Runtime endpoint / store path を含めない。
- 成功後は作成 Worker の Console に遷移し、失敗時は sanitized diagnostic を form に表示する。


---

<!-- event: decision author: hare at: 2026-07-02T13:06:46Z -->

## Decision

Manual Coding Worker create form fields を明確化した。

決定:
- v0 form は `display_name`、`runtime_id`、`profile`、`initial_text` の 4 field に限定する。
- `kind = coding` は endpoint / Backend 側 launch mode として扱い、form input や Browser-facing payload field にはしない。
- `profile` は Browser 自由入力ではなく、Backend が公開する候補から選ぶ。
- Browser-facing request は `runtime_id` / `display_name` / `profile` / `initial_text` にし、`ConfigBundleRef`、requested capabilities、raw path、cwd、secret、Runtime endpoint、store path は含めない。


---

<!-- event: intake_summary author: hare at: 2026-07-02T15:39:49Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-02T15:39:49Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-07-02T16:13:24Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-07-02T17:00:32Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Ticket は Manual Coding Worker launch の product-level API / Sidebar New form / sanitized payload / success Console navigation / unsupported runtime diagnostic を具体化しており、observable な acceptance criteria と validation がある。
- typed relation blocker は 0 件、OrchestrationPlan は prior human-gate waiting note のみで、ユーザーから「2つとも消化して」と明示 follow-up があったため human gate は解除された。
- `00001KWHHRTM9` の Runtime connection management と UI/API surface が近く、別 worktree 並列では衝突リスクが高いので、同一 implementation worktree/branch で `00001KWHHRTM9` を先に実装し、その Runtime candidate/projection を利用して本 Ticket を続けて実装する。

Evidence checked:
- Ticket body / thread / artifacts。
- `TicketRelationQuery(00001KWHEM8YJ)` は 0 件。
- `TicketOrchestrationPlanQuery(00001KWHEM8YJ)` は prior human-gate waiting note のみ。
- queued Ticket 一覧では `00001KWHHRTM9` も queued、ready/inprogress は 0 件。
- workspace/orchestration git state と worktree 一覧、visible Pods、TicketDoctor（0 errors / 既存 warning のみ）。
- Bounded code map: `web/workspace/src/lib/workspace-sidebar/WorkersNavSection.svelte`、console route under `web/workspace/src/routes/runtimes/[runtimeId]/workers/[workerId]/console/`、existing `/api/workers` list projection、`/api/runtimes/{runtime_id}/workers` internal-ish Runtime spawn path、`crates/workspace-server` runtime/backend/config areas。

IntentPacket:

Intent:
- Workspace Browser sidebar の WORKER heading 横に `New` button と作成 form を追加し、Browser-facing `/api/workers` product-level endpoint から coding Worker を作成し、成功後に Worker Console へ遷移する。

Binding decisions / invariants:
- Browser UI は existing Runtime create payload を直接露出しない。
- Browser-facing request fields は `runtime_id` / `display_name` / `profile` / `initial_text` のみ。`kind` は endpoint/backend launch mode として扱い、form input や request field にしない。
- Browser-facing payload/response/form に raw workspace path、cwd、tool scope、`ConfigBundleRef`、requested capabilities、secret/token、Runtime endpoint、socket/session/store path を含めない。
- `profile` は Backend が公開する候補から選ばせ、自由入力にしない。
- v0 は embedded/local Runtime を primary target とし、remote Runtime の workspace provisioning が未対応なら typed diagnostic で拒否してよい。
- `/api/runtimes/{runtime_id}/workers` は残してよいが、New Worker UI は新しい product-level `/api/workers` POST を使う。

Requirements / acceptance criteria:
- Sidebar WORKER heading 横に `New` button がある。
- `New` で display name / runtime / profile / initial text だけの form が開く。
- Form submit は `/api/workers` product-level endpoint を使い、coding Worker を作成できる。
- Backend は request の `profile` から Profile / ConfigBundle / execution backend を解決する。
- 作成成功後、Worker list を refresh し、`/runtimes/{runtime_id}/workers/{worker_id}/console` へ遷移する。
- 作成失敗時、入力値を保持して sanitized diagnostic を form に表示する。
- focused backend/UI tests を追加し、指定 validation を可能な範囲で実行する。

Implementation latitude:
- Runtime candidate は `00001KWHHRTM9` の projection を利用する。v0 で embedded-only fallback が必要なら許容されるが、Browser 自由入力にはしない。
- Product-level endpoint の internal mapping、response shape、Svelte form placement（inline/sidebar panel/modal）は既存 UX を壊さない範囲で選んでよい。
- Profile candidate の v0 source は backend-provided static/builtin candidates でもよい。ただし自由入力化しない。

Escalate if:
- Browser-facing API に raw path/cwd/scope/secret/runtime internal location を含めないと実装できない場合。
- Profile/ConfigBundle resolution の既存 boundary を変える必要が出た場合。
- remote runtime provisioning を完成させないと acceptance を満たせない場合。

Validation:
- `cd web/workspace && deno task test`
- `cd web/workspace && deno task check`
- `cargo test -p yoi-workspace-server`
- `cargo check -p yoi`
- `git diff --check`
- `nix build .#yoi --no-link` は時間/依存変更の重さを見て実行、未実行なら理由を report。

Current code map:
- Sidebar UI: `web/workspace/src/lib/workspace-sidebar/WorkersNavSection.svelte`, `WorkspaceSidebar.svelte`。
- Worker console/navigation: `web/workspace/src/lib/workspace-console/model`, `web/workspace/src/routes/runtimes/[runtimeId]/workers/[workerId]/console/`。
- Existing worker list and runtime spawn APIs: `crates/workspace-server` `/api/workers` and `/api/runtimes/{runtime_id}/workers` handlers, worker-runtime `CreateWorkerRequest` / config bundle resolution paths。
- Runtime candidates/settings base: `00001KWHHRTM9` implementation in the same worktree.

Critical risks / reviewer focus:
- Product-level endpoint does not deserialize or expose the internal Runtime create request shape.
- Browser-facing surfaces are sanitized and do not leak raw path/cwd/scope/ConfigBundleRef/requested capabilities/secret/runtime endpoint/socket/session/store path.
- Profile/runtime selectors are bounded to backend-published choices.
- Success navigation and worker list refresh are deterministic.
- The combined implementation with `00001KWHHRTM9` keeps runtime connection management and manual worker launch contracts consistent.

---

<!-- event: state_changed author: orchestrator at: 2026-07-02T17:01:11Z from: queued to: inprogress reason: routing_acceptance_implementation_ready field: state -->

## State changed

Queued acceptance recorded after explicit user follow-up 「2つとも消化して」。

Checked context:
- Ticket body / thread / artifacts。
- `TicketRelationQuery(00001KWHEM8YJ)`: blocking relation 0 件。
- `TicketOrchestrationPlanQuery(00001KWHEM8YJ)`: prior human-gate waiting note を確認し、今回 accepted_plan / after ordering を記録済み。
- workspace/worktree/visible Pod/TicketDoctor/code-map の bounded check。

Acceptance basis:
- concrete missing decision / information は残っていない。
- `00001KWHHRTM9` と surface が重なるため、同一 implementation worktree で `00001KWHHRTM9` の後に実装する。
- side effect はこの `queued -> inprogress` acceptance 後に開始する。

---

<!-- event: implementation_report author: orchestrator at: 2026-07-02T17:02:43Z -->

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
- Work order remains: `00001KWHHRTM9` first, then this Ticket; external review will be requested after coder completion evidence is available.

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

<!-- event: implementation_report author: orchestrator at: 2026-07-02T18:11:11Z -->

## Implementation report

Review-fix implementation report:

- Coder Pod completed requested changes and committed follow-up `47ed0ff8 fix: harden runtime and worker launch controls` on branch `work/00001KWHHRTM9-00001KWHEM8YJ`.
- Worktree status was clean after commit.
- Fix changed `crates/workspace-server/src/server.rs`, `web/workspace/deno.json`, `WorkersNavSection.svelte`, and added `worker-launch.ts` / `worker-launch.test.ts`.
- Reported fixes include lightweight runtime compatibility probes, fixed sanitized remote-test diagnostics without endpoint/path leakage, preservation of typed unsupported worker-create diagnostics, `deny_unknown_fields` on Browser-facing request structs, empty display-name default derivation, backend route/acceptance tests, and web worker-launch model tests.
- Orchestrator reran validation:
  - `git diff --check`: pass
  - `cargo test -p yoi-workspace-server`: pass（55 lib tests + 2 main tests）
  - `cargo check -p yoi`: pass
  - `cd web/workspace && deno task test`: pass（13 tests）
  - `cd web/workspace && deno task check`: pass（0 errors / 0 warnings）
- `nix build .#yoi --no-link` は dependency/resource packaging 変更ではないため未実行。
- Requesting follow-up external review against `f2fead7e..47ed0ff8` and full combined implementation.

---
