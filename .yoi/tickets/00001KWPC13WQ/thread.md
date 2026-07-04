<!-- event: create author: "yoi ticket" at: 2026-07-04T10:50:45Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-07-04T10:59:11Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-04T10:59:11Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-07-04T19:34:04Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-07-04T19:35:39Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Ticket は Workspace Backend の Repository registry を config-driven にする範囲、非目標、受け入れ条件が具体化されている。
- `WorkspaceBackendConfigFile` / `.yoi/workspace-backend.local.toml` に `[[repositories]]` を追加し、Browser/API が configured repositories のみを返すという binding decision が明確。
- `RepositoryId` / `RepositorySelector` / `RepositoryPoint` の用語境界と、secret/path leak を避ける Browser-facing invariant が明記されている。
- typed relation blocker は 0 件、OrchestrationPlan record は 0 件。
- queued notification は human authorized routing であり、今回の routing acceptance 後にのみ implementation side effect へ進める。

Evidence checked:
- Ticket body / thread / artifacts。
- `TicketRelationQuery(00001KWPC13WQ)`: 0 件。
- `TicketOrchestrationPlanQuery(00001KWPC13WQ)`: 0 件。
- Orchestrator worktree git status: clean on `orchestration`。
- queued Ticket 一覧: この Ticket 1件のみ。ready / inprogress は 0 件。
- visible Pods: 2 visible pods（既存 visible state の確認のみ、spawn は未実施）。
- `TicketDoctor`: 0 errors / 既存 diagnostics のみ。
- Bounded code map: `crates/workspace-server/src/config.rs`, `crates/workspace-server/src/repositories.rs`, `crates/workspace-server/src/server.rs`, `crates/workspace-server/src/store.rs`, repository-related web workspace files。

IntentPacket:

Intent:
- Workspace Backend の Repository view を workspace root/cwd の暗黙 fallback から、`.yoi/workspace-backend.local.toml` の explicit Repository registry に移行する。
- `/api/repositories` と `/api/repositories/{id}/log` は configured repositories のみを扱い、未設定時は empty list + typed warning diagnostic を返す。

Binding decisions / invariants:
- `--workspace` は Repository root ではなく workspace config root / local descriptor root として扱う。
- `uri = "."` は backend process cwd ではなく workspace config root 相対として解釈する。
- cwd / workspace root を暗黙 Repository として Browser/API に返す fallback は廃止する。
- dogfood workspace config には `id = "main"`, `provider = "git"`, `uri = "."` 相当の explicit Repository entry を追加する。
- v0 provider は `git` 主対象。`local_fs` は必要なら placeholder/diagnostic 程度。
- `RepositorySelector` は provider-specific な未解決 locator、`RepositoryPoint` は解決済み evidence として、Git branch/tag/hash 固定抽象にしない。
- Browser-facing response は token/auth ref/secret、不要な absolute host path、internal config/store/runtime paths を漏らさない。

Requirements / acceptance criteria:
- `.yoi/workspace-backend.local.toml` に `[[repositories]]` を書ける。
- `/api/repositories` は configured repositories のみを返す。
- Repository 未設定 workspace は empty list + `repository_config_empty` 相当 warning diagnostic。
- configured Git Repository では branch/head/status/log など既存 UI が必要とする summary を取得できる。
- `/api/repositories/{id}/log` は configured Git Repository id のみ受け付け、未知 id / unsupported provider は typed sanitized error。
- 既存 web UI は未設定時にも壊れず、明示 Repository がある場合だけ repository view を表示できる。

Implementation latitude:
- 型名、module分割、API response shape の細部は既存 `workspace-server` / web workspace style に合わせてよい。
- config TOML の comments/format preservation は既存 local config serializer の範囲で扱う。
- Browser-facing path 表示は safe display name / sanitized summary に寄せ、必要なら absolute path を API 内部で使っても response に不要露出しない。
- focused tests の構成は backend helper/route-level と web model/component-levelの現実的な範囲で選んでよい。

Escalate if:
- credential/clone/fetch/write operation、Runtime materialization、Execution Workspace 作成、multi-workspace store 移行、Ticket target からの RepositoryPoint 解決が必要になる場合。
- Browser-facing API に secret/auth ref/internal absolute paths を出さないと実装できない場合。
- `RepositorySelector` / `RepositoryPoint` の概念境界を設計変更する必要が出た場合。

Validation:
- `cargo test -p yoi-workspace-server`
- `cargo check -p yoi`
- `cd web/workspace && deno task check && deno task test`
- `git diff --check`
- `yoi ticket doctor`
- `nix build .#yoi --no-link` は Cargo.lock / dependency / resource packaging / Nix 変更がある場合だけ実行し、未実行なら理由を報告する。

Current code map:
- Config: `crates/workspace-server/src/config.rs` (`WorkspaceBackendConfigFile`, local config load/write/resolve)。
- Repository projection: `crates/workspace-server/src/repositories.rs` (`LocalRepositoryReader`, `RepositorySummary`, git inspect/log)。
- API wiring: `crates/workspace-server/src/server.rs` (`/api/repositories`, `/api/repositories/{repository_id}`, log route)。
- Store/schema context: `crates/workspace-server/src/store.rs` repositories table migration context。
- Web: workspace repository panels/routes using `/api/repositories` and repository log summaries.

Critical risks / reviewer focus:
- No implicit workspace-root/cwd repository fallback remains in Browser/API output.
- configured repository ids, unknown ids, and unsupported providers fail closed with typed sanitized diagnostics.
- path/secret/internal config/store/runtime details do not leak to Browser-facing responses.
- `uri = "."` is resolved against workspace config root, not process cwd.
- dogfood `.yoi/workspace-backend.local.toml` contains explicit `main` repository entry without introducing secret/local-only fields into tracked records.

---

<!-- event: state_changed author: orchestrator at: 2026-07-04T19:35:52Z from: queued to: inprogress reason: routing_acceptance_implementation_ready field: state -->

## State changed

Queued acceptance recorded after Workspace Dashboard Queue notification authorized Orchestrator routing.

Checked context:
- Ticket body / thread / artifacts。
- `TicketRelationQuery(00001KWPC13WQ)`: blocking relation 0 件。
- `TicketOrchestrationPlanQuery(00001KWPC13WQ)`: prior record 0 件。今回 accepted_plan を記録済み。
- Orchestrator worktree git status: clean on `orchestration`。
- queued Ticket はこの Ticket 1件のみ。ready / inprogress は 0 件。
- visible Pods / TicketDoctor / bounded code map を確認。

Acceptance basis:
- concrete missing decision / information は残っていない。
- Ticket の scope は explicit Repository registry / API projection / web compatibility に閉じており、非目標も明確。
- side effect はこの `queued -> inprogress` acceptance 後に、worktree-workflow に沿って implementation worktree を作成してから開始する。

---

<!-- event: implementation_report author: orchestrator at: 2026-07-04T19:36:56Z -->

## Implementation report

Implementation routing update:

- Worktree-workflow に従い implementation worktree を作成した。
  - Worktree: `/home/hare/Projects/yoi/.worktree/00001KWPC13WQ-repository-registry`
  - Branch: `work/00001KWPC13WQ-repository-registry`
- Worktree には tracked `.yoi` project records が見えている。
- `.yoi/memory` / `.yoi/runtime` / `.yoi/log` / `.yoi/logs` / `.yoi/local` / `.yoi/*.lock` / `.yoi/*secret*` は作成直後に存在しないことを確認した。
- Sibling Coder Pod `yoi-coder-00001KWPC13WQ-repository-registry` を起動した。
- Scope policy: runtime workspace root は read-only、implementation worktree は write 可。Coder task では root/original workspace での read/write/validation/git operation を禁止し、この implementation worktree だけを作業対象に指定した。
- 完了後は diff / validation evidence を Orchestrator が確認し、別 sibling Reviewer Pod に read-only review を依頼する。`StopPod` は既知の不具合があるため使用しない。

---
