<!-- event: create author: "yoi ticket" at: 2026-06-25T19:32:38Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-06-25T19:56:23Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-25T19:56:23Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-25T20:34:42Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: yoi-orchestrator at: 2026-06-25T20:36:43Z -->

## Decision

Routing decision: blocked_by_dependency_or_missing_authority

Reason:
- Dashboard Queue 後に Ticket / relations / workspace state を確認した。
- 本 Ticket は Runtime API / WebSocket observation への TUI移行であり、Backend RuntimeRegistry foundation、embedded/remote Runtime routing、WebSocket observation proxy が前提。
- 現在 `00001KVZKSV6C` は inprogress、`00001KVZKSTJT` / `00001KVZSGT0Q` / `00001KVZSGT14` は queued/blocked。TUI migration を先に始めると transport/API の未確定部分を TUI 側で固定してしまうため開始しない。

Evidence checked:
- Ticket body: TUI connection model、input path、output/observation path、Runtime WebSocket / Backend proxy reliance、compatibility/debug path。
- Relations: outgoing dependencies include `00001KVZKSTJT`, `00001KVZKSV6C`, `00001KVZSGT0Q`, `00001KVZSGT14`。
- Orchestration plan: blocker record `orch-plan-20260625-203613-1` を追加。

Next action:
- 本 Ticket は queued のまま待機。
- Backend RuntimeRegistry / embedded+remote Runtime / WS proxy chain が done になった後に再 routing する。

---

<!-- event: decision author: yoi-orchestrator at: 2026-06-26T08:10:06Z -->

## Decision

Routing decision: implementation_ready

Reason:
- All blocking dependencies are done: WebSocket observation proxy, RuntimeRegistry foundation, embedded Runtime connection, remote Runtime connection, REST command server, config bundle sync, and Web Console MVP.
- Current `inprogress` is 0 and this is the only queued Ticket.
- Ticket body has clear connection model, input path, output/observation path, compatibility/migration boundaries, acceptance criteria, and validation requirements.

Evidence checked:
- Ticket body: TUI Runtime/Worker target identity, Backend API client model, input command path, WebSocket observation path, existing TUI model relation, compatibility/debug path, acceptance criteria。
- Relations: depends_on `00001KVZKSTJT`, `00001KVZKSV6C`, `00001KVZSGT0Q`, `00001KVZSGT14`; all are done. Related REST command server is done.
- Orchestration plan: accepted plan `orch-plan-20260626-080943-5` recorded.
- Workspace state: orchestration worktree clean; no spawned child Workers currently active.

IntentPacket:

Intent:
- TUI の正規接続経路を Backend Runtime API / WebSocket observation stream に移行し、`runtime_id + worker_id` を対象 identity として input/output/status を扱う。

Binding decisions / invariants:
- TUI は remote Runtime endpoint / token / raw socket path / raw session path を authority として扱わない。
- Backend RuntimeRegistry / routing / endpoint credential 管理を TUI 内部に実装しない。TUI は Backend API client として振る舞う。
- Legacy direct socket attach を残す場合は compatibility/debug path として明確に分離し、正規 path と混同しない naming/diagnostics にする。
- Runtime event adapter は既存 Console model へ変換するが、raw provider trace / raw full session log を authority にしない。
- Full auth/multi-user permission model、raw session storage migration、旧 socket protocol 完全互換は Non-goals。

Requirements / acceptance criteria:
- TUI が `runtime_id + worker_id` target で接続できる。
- Input は Backend/Runtime command API 経由で Worker に届く。
- Output/status/transcript update は Runtime/Backend-proxied WebSocket observation stream から受け取る。
- Runtime events が existing TUI Console model に変換され、user message / assistant output / status / error が表示される。
- Initial transcript/snapshot 相当を表示できる。
- Reconnect / cursor resume / duplicate event は基本実装、または typed diagnostic になる。
- Browser/remote Runtime credential/socket/session path を TUI が authority として扱わない。
- Focused TUI/adapter tests が追加される。

Implementation latitude:
- CLI flag/selector UX、Backend API client module placement、Runtime event to Console block adapter design、cursor/reconnect policy は Coder が既存 TUI architecture に合わせて選べる。
- v0 は Backend API が提供する known Runtime/Worker projection に合わせ、dogfoodingに必要な legacy compatibility/debug modeを明示的に残してよい。
- Existing rendering/composer/status components は可能な範囲で再利用。

Escalate if:
- TUI に Runtime endpoint/token/socket/session path を直接渡す必要が出る。
- Backend API/WS が TUI migration に不足し、server foundation の大幅追加が必要になる。
- Existing Console rendering semantics を大きく削る必要がある。
- Pseudo-runtime adapter で userに実 runtime接続と誤認させる必要が出る。

Validation:
- `cargo fmt --all`
- `cargo test -p tui` または該当 TUI crate tests
- `cargo check -p yoi`
- `git diff --check`
- 可能なら `nix build .#yoi --no-link`

Critical risks / reviewer focus:
- Backend/Runtime credential/path leak。
- `worker_id`-only or legacy socket authority regression。
- Runtime event adapter dropping/duplicating transcript events。
- Cursor/reconnect duplicate handling。
- Legacy debug path accidentally becoming the default path。
- Terminal UI regressions that need real TTY/manual validation notes。

---

<!-- event: state_changed author: yoi-orchestrator at: 2026-06-26T08:10:15Z from: queued to: inprogress reason: routing_accepted_after_runtime_ui_foundation_done field: state -->

## State changed

Routing decision: implementation_ready。

WebSocket observation proxy、RuntimeRegistry foundation、embedded/remote Runtime connection、REST command server、config bundle sync、Web Console MVP は done。Ticket body / relations / workspace state / accepted plan を確認し、TUI Runtime API / WebSocket migration は unblocked と判断した。accepted plan と IntentPacket は thread / orchestration plan に記録済み。

これ以降、worktree creation / coder Worker routing などの implementation side effect に進める。

---

<!-- event: implementation_report author: yoi-orchestrator at: 2026-06-26T08:11:15Z -->

## Implementation report

Implementation kickoff:

- Worktree created: `/home/hare/Projects/yoi/.worktree/00001KW04A8K6-tui-runtime-api`
- Branch: `work/00001KW04A8K6-tui-runtime-api`
- Coder Worker spawned: `yoi-coder-00001KW04A8K6-tui-runtime`

Scope notes:
- Coder has read access to the implementation worktree and non-recursive workspace root identity path needed by runtime startup.
- Coder write scope is limited to `crates/tui/**`, `crates/client/**`, `crates/protocol/**`, `crates/yoi/**`, `crates/workspace-server/**`, root `Cargo.toml`, `Cargo.lock`, and `package.nix` if dependency/package changes are needed.
- Coder is instructed not to modify Ticket records and not to operate in root/orchestration worktrees.

Queue status:
- This was the last queued Ticket in the current chain; no other queued Ticket remains pending in this acceptance pass.

---

<!-- event: implementation_report author: yoi-coder-00001KW04A8K6-tui-runtime at: 2026-06-26T08:43:52Z -->

## Implementation report

Implementation report:

実装完了。merge/close/cleanup は行っていない。

Commit:
- `63ec9f95 feat: add backend runtime console target`

変更ファイル:
- `Cargo.lock`
- `crates/client/Cargo.toml`
- `crates/client/src/backend_runtime.rs`
- `crates/client/src/lib.rs`
- `crates/tui/src/console/mod.rs`
- `crates/tui/src/lib.rs`
- `crates/yoi/src/main.rs`
- `package.nix`

実装概要:
- `client` crate に Backend Runtime API 用クライアントを追加。
  - authority は `Backend API base URL + runtime_id + worker_id`。
  - Runtime endpoint / token / raw socket path / raw session path は TUI 側の authority として扱っていない。
  - `POST /api/runtimes/{runtime_id}/workers/{worker_id}/input` で user/system input を送信。
  - `POST .../cancel` / `POST .../stop` で cancel/stop を送信。
  - `GET .../transcript` から初期 transcript 相当を TUI event に変換。
  - `WS .../events/ws` で Backend-proxied Runtime observation stream を購読。
  - cursor resume / duplicate sequence drop / cursor expiry diagnostic + reset/reconnect を実装。
  - Backend diagnostic / unsupported command / HTTP decode/send failure は TUI に `Event::Error` として表示。
- TUI Console に Backend Runtime 接続モードを追加。
  - `LaunchMode::BackendRuntime { target }`
  - 既存 Console rendering/composer/status の `App::handle_worker_event` へ `protocol::Event` を流して再利用。
  - legacy socket path は `ConsoleConnection::LegacySocket` として明示分離。
- CLI に Backend Runtime target selector を追加。
  - `--backend <URL>`
  - `--runtime-id <ID>` / alias `--runtime <ID>`
  - `--worker-id <ID>`
  - 3つは必須セット。
  - `--worker` / `--socket` / `--session` / `--profile` との混在を拒否。
  - help 上で `--socket` は compatibility/debug direct socket attach と明記。
- Focused tests を追加。
  - Backend command mapping。
  - Backend WS URL generation using `runtime_id + worker_id`。
  - Transcript projection → TUI Console event sequence。
  - CLI Backend Runtime target parsing / incomplete identity rejection / legacy option mix rejection。

Validation:
- `cargo fmt --all`: success
- `cargo test -p tui`: success（382 passed）
- `cargo test -p yoi parse_backend_runtime_target`: success（3 passed）
- `cargo test -p client backend_runtime::tests`: success（3 passed）
- `cargo check -p yoi`: success
- `git diff --check` / `git diff HEAD --check`: success
- `nix build .#yoi --no-link`: success

Additional note:
- `cargo test -p client` 全体も一度実行したが、既存の `ticket_role::tests::scaffold_config_allows_intake_and_orchestrator_launch_planning` が Lua builtin profile 評価エラーで失敗。今回追加した `backend_runtime::tests` は個別に成功。

Residual risks / review focus:
- Backend Runtime mode は既存 Backend endpoint 形状に合わせた薄い API client 実装で、Runtime routing / endpoint credential management は TUI 側に入れていない。
- pause/resume/compact/rewind/completion lookup 等、Backend API にまだ対応 endpoint がない TUI commands は diagnostic 表示のみで送信しない。
- 初期 transcript は Backend transcript projection から user/assistant chat event に変換。raw session log / provider trace は authority として使っていない。
- default local spawn / legacy Worker-name attach 経路は既存互換として残る。Backend Runtime target 経路は `--backend + --runtime-id + --worker-id` による明示接続。

---
