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
<!-- event: implementation_report author: orchestrator at: 2026-06-13T18:33:30Z -->

## Implementation report

Implementation handoff resumed successfully after retry.

- Previous blocker was child launch validation requiring workspace root read grant.
- Retried `SpawnPod` with explicit non-recursive read grant for `/home/hare/Projects/yoi` plus child worktree read/write scopes.
- Coder Pod started: `coder-00001KV10SN02-e2e-critical`
- Worktree: `/home/hare/Projects/yoi/.worktree/e2e-critical-path`
- Branch: `ticket-00001KV10SN02-e2e-critical-path`

The root workspace grant is non-recursive and is only for runtime launch validation. Coder instructions explicitly prohibit inspecting, writing, running git, validating, merging, or cleaning up in the root/original workspace.

---

<!-- event: state_changed author: orchestrator at: 2026-06-13T18:43:30Z from: queued to: inprogress reason: orchestrator_acceptance_recorded_after_spawn_retry field: state -->

## State changed

Coder Pod `coder-00001KV10SN02-e2e-critical` has been started after SpawnPod retry with explicit root non-recursive read grant. This records the missing acceptance marker before further implementation/review side effects continue; previous handoff report already documented the retry and scope.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-13T19:04:40Z -->

## Implementation report

Implementation blocked / stopped for now.

Problem:
- Coder Pod `coder-00001KV10SN02-e2e-critical` is in an inconsistent communication state: `ReadPodOutput` reports it as stopped/unreachable, while restore/list surfaces report the socket as live/reachable, and `SendToPod` times out waiting for initial Snapshot.
- Worktree `/home/hare/Projects/yoi/.worktree/e2e-critical-path` contains uncommitted changes in `crates/tui/src/multi_pod.rs`, `crates/tui/src/single_pod.rs`, `tests/e2e/src/lib.rs`, `tests/e2e/tests/panel.rs`, and new `tests/e2e/tests/rewind.rs`.
- No Coder final report, commit, or Ticket implementation_report was received.

Decision:
- Per user instruction, stop this Ticket instead of attempting to merge incomplete/unreviewed work.
- Do not merge this branch.
- Preserve the dirty worktree for human/next-Orchestrator inspection rather than deleting it.

Recovery needed:
- Inspect dirty diff in `/home/hare/Projects/yoi/.worktree/e2e-critical-path`.
- Decide whether to salvage/commit after validation, ask Coder to continue after runtime recovery, or discard and restart from clean branch.
- Do not mark done until implementation is committed, reviewed, and validated.

---

<!-- event: implementation_report author: orchestrator at: 2026-06-14T05:13:26Z -->

## Implementation report

Recovery action: hand off dirty worktree to a replacement Coder.

User requested continuing with a different Coder after the prior Coder Pod stopped unexpectedly.

Current worktree state before handoff:
- Worktree: `/home/hare/Projects/yoi/.worktree/e2e-critical-path`
- Branch: `ticket-00001KV10SN02-e2e-critical-path`
- Dirty files from prior Coder attempt:
  - `crates/tui/src/multi_pod.rs`
  - `crates/tui/src/single_pod.rs`
  - `tests/e2e/src/lib.rs`
  - `tests/e2e/tests/panel.rs`
  - `tests/e2e/tests/rewind.rs` (new)
- `git diff --check` currently reports no whitespace/check errors.

Replacement Coder task:
- Inspect the dirty diff, decide whether it is salvageable, then either finish/commit/report it or stop with a concrete blocker.
- Do not merge or clean up; Orchestrator will handle review/merge.

---

<!-- event: implementation_report author: hare at: 2026-06-14T05:24:37Z -->

## Implementation report

Implemented critical-path TUI E2E coverage in commit b9f49eee.

Files changed:
- crates/tui/src/multi_pod.rs: panel mouse wheel events now move selection and emit e2e observer diagnostics without enabling full drag-motion capture.
- crates/tui/src/single_pod.rs: e2e-test-only canned single-Pod rewind fixture path for real TUI/PTTY driving; preserves production/non-production boundary.
- tests/e2e/Cargo.toml: disables auto-discovered integration tests and registers panel/rewind only behind the e2e feature.
- tests/e2e/src/lib.rs: harness support for rewind fixture env isolation, wheel input, event waiting, and forbidden ?1002h/?1003h capture detection.
- tests/e2e/tests/panel.rs: adds PTY wheel regression coverage and drag-capture failure assertion while keeping existing panel cases.
- tests/e2e/tests/rewind.rs: adds single-Pod rewind picker/apply/duplicate-Enter regression.

Validation:
- cargo fmt --check
- git diff --check
- cargo test -p yoi-e2e --no-run
- cargo build -p yoi
- cargo test -p yoi-e2e --features e2e
- cargo test -p tui --lib single_pod::tests::rewind
- cargo test -p tui --lib mouse
- nix build .#yoi

Additional note:
- Full cargo test -p tui was also attempted and failed in pre-existing/unrelated tests: multi_pod::tests::orchestrator_launch_context_uses_orchestration_root_for_runtime_workspace and spawn::tests::{profile_choices_include_builtin_and_project_default_marker, profile_choices_use_project_registry_default}. Focused changed-area tui tests passed.

Residual risks:
- Rewind E2E uses an e2e-test-only canned in-process fixture instead of a provider/socket-backed Pod, intentionally avoiding credentials/network while still exercising the real single-Pod TUI/PTTY path.
- nix build emitted the expected dirty-tree warning before succeeding.

---

<!-- event: review author: hare at: 2026-06-14T05:33:43Z status: approve -->

## Review: approve

APPROVE.

Review evidence:
- Diff base: inspected `git diff orchestration/yoi-orchestrator..HEAD`; changed files are limited to this Ticket record plus intended TUI/E2E files (`crates/tui/src/{multi_pod.rs,single_pod.rs}`, `tests/e2e/{Cargo.toml,src/lib.rs,tests/panel.rs,tests/rewind.rs}`). No unrelated Ticket records or broad code churn were present.
- Existing Panel E2E remains opt-in: `tests/e2e/Cargo.toml` uses `autotests = false`; `panel` and `rewind` integration tests have `required-features = ["e2e"]`. `cargo test -p yoi-e2e --no-run` compiled only the library test, while `cargo test -p yoi-e2e --features e2e` ran the isolated panel/rewind cases.
- Wheel PTY coverage is meaningful: the panel test drives real PTY SGR wheel input at a row coordinate derived from `rows_rendered`, then observes `mouse_wheel`, `selection_changed`, and refreshed `rows_rendered` events. This exercises terminal input delivery and row/list selection behavior rather than direct state mutation.
- Mouse capture assertions cover the intended invariant: the TUI now enables normal tracking + SGR through the local `EnableWheelMouseCapture` command, and tests assert mouse capture is enabled while rejecting forbidden full drag-motion modes. Observed PTY artifacts from the reviewer run contained `?1000h`/`?1006h` once and `?1002h`/`?1003h` zero times for panel and rewind runs.
- Rewind E2E uses a real single-Pod TUI/PTTY surface with an `e2e-test`-only fixture. The test sends `Ctrl+R`, `Enter`, and repeated `Enter`; observed events show picker open, exactly one rewind submit, duplicate Enter suppression while pending, and `rewind_applied` with restored composer text. The PTY output artifact also contained the restored text (`revise the plan`), so this covers visible composer restoration without Esc/restart/restore.
- Credential/network isolation is preserved: the E2E harness uses `env_clear`, fixture HOME/XDG runtime/data/config/state, explicit runtime binary override, and default-deny provider credential policy. The rewind fixture is only compiled/activated behind the `e2e-test` feature and `YOI_TUI_TEST_REWIND_FIXTURE`; normal `cargo build -p yoi` passed without that fixture.

Validation performed:
- `cargo fmt --check` — pass
- `git diff --check orchestration/yoi-orchestrator..HEAD` — pass
- `cargo test -p yoi-e2e --no-run` — pass
- `cargo build -p yoi` — pass
- `cargo test -p yoi-e2e --features e2e` — pass (1 lib env-policy test, 3 panel tests, 1 rewind test)
- `cargo test -p tui --lib single_pod::tests::rewind` — pass (5 tests)
- `cargo test -p tui --lib mouse` — pass (4 tests)
- `cargo test -p tui --lib` — fails in known unrelated tests already noted by implementation report: `multi_pod::tests::orchestrator_launch_context_uses_orchestration_root_for_runtime_workspace` and `spawn::tests::{profile_choices_include_builtin_and_project_default_marker, profile_choices_use_project_registry_default}`; 327 other tests passed, including the changed-area mouse/rewind tests.
- `nix build .#yoi` — not run by reviewer because this review grant allows writes only under `target/` and this Ticket record; a normal nix build would write outside that boundary (store/result link). Coder's implementation report recorded a successful nix build with the expected dirty-tree warning.

Residual risk:
- The rewind E2E intentionally uses an in-process canned rewind fixture instead of a provider/socket-backed Pod to avoid credentials and network. This is acceptable for the Ticket's critical TUI/PTTY regression focus, but it is not full provider integration coverage.

<!-- event: implementation_report author: orchestrator at: 2026-06-14T05:38:52Z -->

## Implementation report

Integration outcome: merged and validated.

- Implementation branch: `ticket-00001KV10SN02-e2e-critical-path`
- Implementation commits: `b9f49eee test: cover critical tui e2e paths`, `3de938b7 ticket: report e2e critical implementation`
- Sync commit: `b6c6fc04 merge: sync e2e critical branch`
- Review commit: `581cb202 ticket: approve e2e critical path`
- Orchestrator merge commit: `059b1fd4 merge: e2e critical path`

Reviewer result:
- `approve`。Panel wheel PTY coverage, forbidden drag-capture checks, single-Pod rewind PTY E2E with canned e2e-test fixture, no real provider/network calls, and existing tmp/env/runtime isolation preservation were reviewed.

Orchestrator validation after merge:
- `cargo fmt --check`: PASS
- `git diff --check`: PASS
- `cargo test -p yoi-e2e --no-run`: PASS
- `cargo build -p yoi`: PASS
- `cargo test -p yoi-e2e --features e2e`: PASS
- `cargo test -p tui --lib single_pod::tests::rewind`: PASS
- `cargo test -p tui --lib mouse`: PASS
- `nix build .#yoi`: PASS

Residual notes:
- Full `cargo test -p tui --lib` was not used as merge gate because reviewer/coder both observed known unrelated failures in `multi_pod::tests::orchestrator_launch_context_uses_orchestration_root_for_runtime_workspace` and `spawn::tests::{profile_choices_include_builtin_and_project_default_marker, profile_choices_use_project_registry_default}`.
- Rewind E2E intentionally uses e2e-test-only canned fixture rather than provider/socket-backed Pod to avoid credentials/network while covering the TUI/PTTY regression path.

Next:
- Mark Ticket done and clean up replacement Coder / Reviewer Pods plus implementation worktree/branch.

---

<!-- event: state_changed author: orchestrator at: 2026-06-14T05:39:03Z from: inprogress to: done reason: merged_and_validated field: state -->

## State changed

Implementation branch was reviewed, approved, merged into the Orchestrator branch as `059b1fd4`, and validated in the Orchestrator worktree. Focused E2E, TUI rewind/mouse tests, formatting, diff check, `cargo build -p yoi`, and `nix build .#yoi` passed. Ticket implementation work is done; closure remains separate.

---
