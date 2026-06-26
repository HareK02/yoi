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
