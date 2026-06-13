<!-- event: create author: "yoi ticket" at: 2026-06-13T17:34:41Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: decision author: hare at: 2026-06-13T17:39:19Z -->

## Decision

修正:
- 初回作成時は git log の確認が不足しており、既存/別 branch の E2E 実装済み範囲を一部重複して書いていた。
- git log を確認し、既存の Panel E2E harness、latest-binary build、late quit latency E2E、mouse click selection E2E、orchestration branch の tmp runtime/data isolation を前提として反映した。
- この Ticket は残差に絞る: wheel regression、rewind UI real-process regression、必要最小限の critical path 維持/補強。


---

<!-- event: state_changed author: workspace-panel at: 2026-06-13T17:51:04Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-06-13T17:51:56Z -->

## Decision

Routing decision: implementation_ready

Reason:
- Ticket は `queued` で、既存 E2E harness first slice / latest-binary build / env isolation / tmp runtime isolation を前提にした残差 coverage が具体化されている。
- `TicketRelationQuery` の relation は `related` のみで、blocking dependency はない。`TicketOrchestrationPlanQuery` に既存 blocker/conflict 記録はない。
- `00001KV0YK5S0` の tmp runtime/data isolation は Orchestrator branch に merge/validated 済みで、この Ticket の前提は満たされている。
- Risk flags は `e2e` / `tui` / `pty` / `quit-latency` / `mouse-input` / `rewind` だが、binding scope は opt-in E2E の missing critical-path coverage に限定され、real provider/network call 禁止、host credential isolation、fixture runtime isolation などの invariants が明記されている。
- Orchestrator worktree は clean。visible implementation Pods はない。ほか queued Ticket はあるが、Panel/TUI 変更面や broad authority model 変更と同時に進めると review/validation capacity と merge conflict risk が上がるため、この Ticket を先に受理し、他 queued は現時点で開始しない。

Evidence checked:
- Ticket body / thread / artifacts。
- relation records: related links only。
- orchestration plan records: なし。
- related completed work: `00001KSKBP9YG`, `00001KV0TJVN5`, `00001KV0YK5S0`, `00001KV04NJ8D`, `00001KV0723PC`, `00001KV072V89`。
- workspace state: Orchestrator worktree clean、live spawned implementation Pods なし。
- current queue: `00001KV0X254D`, `00001KV09X0XC`, `00001KV0SP0TY`, `00001KV10SN02`。

IntentPacket:

Intent:
- Existing opt-in `yoi-e2e` Panel PTY harness を土台に、larger feature/MCP work 前の remaining critical-path gaps を最小限埋める。
- 既存 Panel quit latency / mouse click selection / tmp isolation coverage は維持し、wheel/drag-capture regression と rewind UI real-process regression を追加する。

Binding decisions / invariants:
- E2E は opt-in のまま。`cargo test --workspace` default に混ぜない。
- No real provider credentials / no network-backed LLM calls。
- Tested processes は clean fixture tmp workspace/data/runtime/env isolation を使い、host runtime/credential を見ない。
- Existing binary provider and `YOI_E2E_BIN` override, env isolation, mouse capture guard, quit pending barrier, cleanup artifacts を壊さない。
- `cargo run` を measured process-under-test にしない。
- Full drag-motion mouse capture (`?1002h` / `?1003h`) を reintroduce しないことを可能な範囲で PTY output から確認する。
- Rewind E2E は success が `Esc` / restart / restore 後でないと見えない状態を fail させる。

Requirements / acceptance criteria:
- Existing Panel smoke/click/quit tests still pass on fixture-isolated harness。
- Mouse E2E covers wheel input for viewport/list behavior and fails if wheel is ignored where observable。
- Mouse E2E fails if click-to-select dispatches action or if full drag-motion capture is observed where forbidden。
- Rewind UI E2E drives real single-Pod TUI or equivalent PTY surface with fixture/canned state: `Ctrl+R` opens picker; target `Enter` updates visible live view/composer/state without requiring `Esc`, restart, or restore; repeated `Enter` while pending does not send duplicate destructive rewind requests。
- Failure artifacts include PTY output/events/status/timing enough for diagnosis。
- Validation includes focused E2E commands, affected checks/tests, `cargo build -p yoi`, and `nix build .#yoi`。

Implementation latitude:
- Coder may extend the existing PanelHarness or add a SinglePodHarness if cleaner。
- Canned session/test-only runtime controls may be used, but must remain opt-in/read-only or test-only and must not enable real provider calls。
- Wheel coverage may use structured events and/or screen artifact assertions, but must exercise PTY input path。
- Rewind scenario can be minimal and fixture-driven; do not broaden into a full provider/protocol E2E matrix。

Escalate if:
- Rewind E2E requires broad public protocol/API changes or real provider calls。
- Single-Pod PTY fixture cannot be built without exposing test-only production surfaces beyond the existing `e2e-test` boundary。
- `nix build .#yoi` requires package/source-filter changes outside delegated scope。

Validation:
- `cargo test -p yoi-e2e --features e2e ...` focused targets。
- `cargo fmt --check`。
- `git diff --check`。
- Relevant package checks/tests, including `cargo build -p yoi`。
- `nix build .#yoi` because package/source-filter correctness may be affected by E2E/test files or Cargo changes。

---

<!-- event: state_changed author: orchestrator at: 2026-06-13T17:52:12Z from: queued to: inprogress reason: orchestrator_acceptance_unblocked_e2e_critical_path field: state -->

## State changed

Ticket evidence、related records、orchestration plan、existing E2E branch state、workspace state、visible Pods を確認した。Blocking relation はなく、tmp runtime isolation 前提は merge 済み。Implementation side effect の前に inprogress acceptance を記録する。

---

<!-- event: implementation_report author: orchestrator at: 2026-06-13T17:54:09Z -->

## Implementation report

Implementation handoff blocked after acceptance.

- Routing / IntentPacket / accepted plan は記録済み。
- State は `queued -> inprogress` acceptance 済み。
- Implementation worktree は作成済み: `/home/hare/Projects/yoi/.worktree/e2e-critical-path`
- Branch: `ticket-00001KV10SN02-e2e-critical-path`

Blocker:
- Coder Pod `coder-00001KV10SN02-e2e-critical` の spawn に失敗した。
- Child launch validation が runtime workspace root `/home/hare/Projects/yoi` の readable grant を要求したが、現在の Orchestrator delegation scope では `/home/hare/Projects/yoi` non-recursive read を子に再委譲できない。
- Error: `failed to create spawned pod: cwd is not readable under the configured scope: /home/hare/Projects/yoi`

Impact:
- まだ Coder は起動しておらず、実装変更は行われていない。
- Worktree creation だけが side effect として存在する。

Next recovery options:
1. Orchestrator Pod を、child launch validation に必要な root workspace non-recursive read delegation を持つ状態で再起動/復旧してから Coder を起動する。
2. 明示許可があれば、この Orchestrator が sibling Coder なしで直接実装する。ただし本 Ticket は E2E/TUI/rewind の critical-path coverage で review 重要度が高いため、可能なら sibling Coder/Reviewer flow を復旧する方が望ましい。
3. 継続できない場合は、作成済み worktree/branch を cleanup し、scope blocker を記録したまま人間判断待ちにする。

No further implementation side effects were performed after the failed spawn.

---
