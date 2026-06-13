<!-- event: migration author: tickets.sh-migration at: 2026-05-27T00:00:02Z -->

## Migrated

Migrated from tickets/e2e-harness.md. No legacy review file was present at migration time.

---

<!-- event: decision author: orchestrator at: 2026-06-13T13:56:37Z -->

## Decision

E2E scope refinement: TUI/Panel PTY 自動化もこの Ticket の範囲に含める。

背景:
- Panel mouse selection / Panel Quit latency の直近不具合では、focused unit test と code-path review だけで `done` 判定し、実端末経路の positive validation / measured validation が不足していた。
- 既存本文の「TUI バイナリを PTY で叩く方針は採らない」は、blind な固定入力スクリプトや GUI 代替としての ad hoc 操作を避ける意図として扱い、TUI/Panel の実プロセス・実端末入力を検証する automated PTY harness は本 Ticket に含める。
- Pod protocol/subprocess E2E と TUI/Panel PTY E2E は harness の部品は違うが、どちらも「実プロセスを spawn して user-visible boundary を検証する」ため、別 umbrella に分けず、この E2E harness Ticket の phase として扱う。

方針:
- 固定 sleep + 固定 input だけの PTY script は採用しない。Harness は UI からの structured feedback を待ってから入力を送る。
- TUI/Panel には test-only / opt-in の observability route を追加する。これは UI action を bypass する command channel ではなく、状態観測・同期・失敗診断のための read-only probe とする。
- 実際の keyboard / mouse / Ctrl+C 入力は PTY 経由で送る。Probe は `first_draw`、`panel_snapshot_ready`、`rows_rendered`、`selection_changed`、`actionbar_changed`、`background_task_started/finished/aborted`、`quit_requested`、`terminal_cleanup_started/finished`、`exit` などの structured event を JSONL 等で吐く。
- Mouse E2E は `rows_rendered` の row key と screen rect を待ち、SGR mouse sequence を PTY に送って、`selection_changed` と screen/actionbar/detail の変化を確認する。
- Quit latency E2E は `panel_ready` / background work pending などの barrier event を待ってから `Ctrl+C` / `Ctrl+D` を送り、`quit_requested -> exit` の elapsed を測る。非本質 background work が abort/drop され、terminal cleanup が行われることも event で確認する。
- Screen output は `vt100`/`vte` 等の terminal parser で secondary oracle / artifact として保存する。主要同期は structured event に寄せる。
- Test probe は `--tui-test-events <path>` 等の明示的な hidden/dev/test flag か `e2e` feature 配下の構成で有効化し、通常実行・model context・Ticket authority・Pod protocol には影響させない。
- Failure artifact として event JSONL、input log、screen dump、stdout/stderr、runtime/data/workspace tmpdir の relevant tree、timing summary を保存する。

受け入れ条件の追加案:
- `cargo test -p e2e --features e2e`（または同等の opt-in command）で実 `yoi panel` を PTY 上で起動し、structured probe feedback を待ってから入力する harness が動く。
- Panel row click E2E: rendered row rect を使って SGR mouse click を送り、selected row が変わることを assertion する。
- Panel quit latency E2E: ready/pending background work barrier 後に Quit 入力を送り、exit latency が閾値内で、nonessential background work が quit を block しないことを assertion する。
- Fixed sleep だけに依存する test は不可。ready/barrier event が来なければ screen dump と event log を artifact として失敗する。
- Probe は read-only observability であり、input/action path を bypass しないことを reviewer が確認する。

---

<!-- event: decision author: orchestrator at: 2026-06-13T14:03:56Z -->

## Decision

E2E design decision: Playwright-like declarative test API と production binary 非混入を前提にする。

Decision:
- E2E は ad hoc shell / fixed sleep script ではなく、Rust の独立 crate から宣言的に scenario を書ける構造にする。
- 例: `PanelHarness::spawn(...)`、`panel.wait_for(PanelReady)`、`panel.click(row("ticket", id))`、`panel.expect_selection(...)`、`panel.press(CtrlC)`、`panel.expect_exit_within(...)` のように、Playwright 的な wait/action/assertion API を提供する。
- Harness crate は production binary / normal library API から独立させる。想定配置は `tests/e2e/` または `crates/e2e_harness` + integration tests で、通常 build / release package / normal `yoi` binary に test harness logic を混ぜない。
- 本番 binary に混ぜる必要があるものは、原則として「既存 TUI state から read-only diagnostic event を emit するための最小 test hook」に限定する。その hook も normal runtime では無効で、明示 feature / hidden dev flag / cfg(test/e2e) 等でしか有効化しない。
- E2E harness は production code の内部関数を直接呼んで state mutation しない。入力は PTY、観測は structured test events / terminal screen parser、assertion は harness 側で行う。
- Structured events は protocol authority ではなく test observability artifact として扱う。Ticket/Pod authority や user-visible semantics を変えない。

Rationale:
- 今回の Panel mouse / Quit latency の失敗は、unit/focused tests と code-path review だけでは user-visible terminal behavior を保証できないことを示した。
- 一方で fixed sleep + input script は再現性・診断性が低く、ready 状態や background work barrier を確認できない。
- Playwright-like API なら、test は「何を待ち、何を入力し、何を観測するか」を宣言的に表現でき、失敗時に event log / screen dump / timing artifact を残せる。
- Production binary への混入を避けることで、release behavior / binary size / authority surface / model-visible surfaces を汚さない。

Acceptance refinement:
- E2E test author が fixed sleep ではなく `wait_for` / `expect` / `within` を使って Panel/TUI scenario を書ける。
- Mouse selection と Quit latency の regression は、この declarative harness API 上の scenario として表現される。
- Test-only observability route は opt-in であり、release/normal execution では無効または到達不能であることを reviewer が確認する。
- Failure artifact に scenario step、last observed events、screen snapshot、timing、binary path、workspace/runtime dirs が含まれる。

---

<!-- event: decision author: orchestrator at: 2026-06-13T14:16:24Z -->

## Decision

Routing decision: implementation_ready

Reason:
- ユーザーが E2E harness を 1 Ticket として扱い、Playwright-like declarative API、structured feedback、production binary 非混入を前提に進めることを明示した。
- Ticket body は旧名/旧構成を含むが、thread decisions により現在の binding direction は明確化済み: Pod subprocess/protocol E2E と TUI/Panel PTY E2E を同じ harness Ticket の phase として扱う。
- 直近の Panel mouse selection / Panel Quit latency の regression から、実プロセス・実 PTY・structured event feedback・failure artifact を最小スライスに含める必要がある。
- `TicketRelationQuery` では durable blocker はなく、関連 Ticket は context link のみ。
- Orchestrator worktree は clean。implementation side effect は state acceptance 後に dedicated child worktree で行う。

Evidence checked:
- Ticket body / thread decisions。
- relation records: `00001KV072V89` / `00001KV0723PC` への related links。
- orchestration plan records: なし。
- current workspace state: Orchestrator worktree clean、queued/inprogress work なし、implementation child Pods なし。
- project context: AGENTS guidance の E2E 未設計、prompt/resource boundary、production binary contamination 回避方針、直近 Panel validation failure records。

IntentPacket:

Intent:
- Yoi の E2E testing foundation を、実プロセス spawn と TUI/Panel PTY automation の両方を扱える opt-in harness として導入する。
- 最初の vertical slice は、Playwright-like declarative API、structured UI feedback、failure artifact、Panel mouse selection / Panel quit latency の regression scenario を実装できる形にする。

Binding decisions / invariants:
- E2E harness は independent crate / test surface とし、normal release / normal `yoi` binary に harness logic を混ぜない。
- 本番 binary 側に必要な変更は opt-in read-only observability hook に限定する。UI action/state mutation を test hook で bypass しない。
- 実入力は PTY 経由で送る。structured event は synchronization / assertion / artifact のための観測情報であり、authority channel ではない。
- fixed sleep + fixed input だけの blind script を acceptance にしない。
- Pod/Ticket authority、prompt/resource boundary、public runtime behavior を E2E 都合で歪めない。

Requirements / acceptance criteria:
- E2E author が Rust code で `spawn` / `wait_for` / `click` / `press` / `expect_*` / `within` を使って scenario を宣言的に書ける。
- Opt-in command（例: `cargo test -p e2e --features e2e` または同等）で通常 CI 既定から分離される。
- TUI/Panel test は panel ready / rows rendered / selection changed / background task / quit events など structured feedback を待ってから PTY input を送る。
- Panel mouse selection regression と Panel quit latency regression の少なくとも skeleton または minimal passing scenario が declarative harness 上で表現される。
- Failure artifact として event log、input log、screen dump、timing、binary path、workspace/runtime dirs が残る。
- Production binary contamination がないこと、または opt-in hook が normal runtime で無効/到達不能であることを reviewer が確認できる。

Implementation latitude:
- `tests/e2e/` crate か `crates/e2e_harness` + integration tests のどちらに置くかは Coder が codebase constraints を見て選んでよい。ただし normal build/release contamination は避ける。
- PTY crate、terminal parser、event JSONL format、fixture workspace builder の具体設計は Coder が選んでよい。
- 最初の slice は full provider E2E ではなく、Panel/TUI harness と minimal process lifecycle / artifact foundation を優先してよい。
- 既存旧名 `INSOMNIA_*` / `pod` references は現在の `yoi` / config surface に合わせて整理してよい。

Escalate if:
- read-only observability hook では足りず、production UI action path を test-only command channel で直接操作したくなる場合。
- normal release binary / normal CLI surface に test-only options を露出させる必要がある場合。
- workspace structure、Cargo package layout、Nix/package source filter に大きな変更が必要になる場合。
- Provider stub / Pod protocol E2E まで同時に広げないと Panel slice が進められない場合。

Validation:
- focused E2E harness tests / example scenarios。
- `cargo fmt --check`。
- `git diff --check`。
- 変更範囲に応じて `cargo check --workspace --all-targets` または narrower package checks。
- 新 E2E command が opt-in で実行可能であることを report する。

Current code map:
- `crates/yoi` / CLI launch path: hidden/test-only flag injection の候補。
- `crates/tui/src/multi_pod.rs`: Panel events / observable state emission の候補。
- `tests/e2e/` or new harness crate: declarative scenario API / PTY runner / artifact collector。
- root `Cargo.toml` / package metadata: opt-in package registration と release contamination check。

Critical risks / reviewer focus:
- Harness code が production binary に混ざっていないこと。
- Observability hook が read-only で、input/action path を bypass していないこと。
- Test が fixed sleep 依存ではなく structured feedback / timeouts / artifacts を持つこと。
- Panel mouse / quit latency regression が今後「unit test だけで done」にならない程度の user-visible path を cover すること。

---

<!-- event: intake_summary author: orchestrator at: 2026-06-13T14:16:54Z -->

## Intake summary

ユーザー確認により、既存 E2E harness Ticket は Pod subprocess E2E と TUI/Panel PTY E2E を一つの実装対象として扱う。Playwright-like declarative API、independent opt-in crate、production binary 非混入、read-only structured observability、PTY input、failure artifact、Panel mouse / quit latency regression scenario が受け入れ方向として明確化済み。

---

<!-- event: state_changed author: orchestrator at: 2026-06-13T14:16:54Z from: planning to: ready reason: user_authorized_e2e_harness_implementation field: state -->

## State changed

Ticket planning が完了しました。state planning -> ready。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-13T14:17:34Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `yoi ticket` が queued にしました。


---

<!-- event: state_changed author: orchestrator at: 2026-06-13T14:17:40Z from: queued to: inprogress reason: orchestrator_acceptance_after_user_authorization field: state -->

## State changed

ユーザーが明示的に inprogress 化して進めることを承認した。Ticket evidence / relation records / OrchestrationPlan accepted plan / Orchestrator worktree clean state を確認済みで、blocking relation はない。Implementation side effect の前に state を inprogress として記録する。

---

<!-- event: implementation_report author: hare at: 2026-06-13T14:38:03Z -->

## Implementation report

Implemented an opt-in E2E testing foundation for real `yoi panel` process automation.

API / harness shape:
- Added workspace package `tests/e2e` (`yoi-e2e`) with required feature `e2e`, so scenarios are opt-in.
- Added `PanelHarness::spawn(...)`, `wait_for(...)`, `wait_for_rows(...)`, `click(...)`, `press(...)`, `expect_selection(...)`, `expect_exit_within(...)`, and `artifacts()`.
- Harness drives the real `yoi panel` binary through a Unix PTY and sends SGR mouse / keyboard bytes through that PTY only.
- Harness captures artifacts under a per-run artifact directory: `events.jsonl`, `input.log`, `pty-output.log`, and `run.json`.

Production / non-production boundary:
- Harness logic stays in `tests/e2e` and is not mixed into production crates.
- Production-side change is limited to an opt-in read-only TUI JSONL observer enabled only by `YOI_TUI_TEST_EVENTS`.
- Observer records Panel/TUI synchronization and assertion events (`panel_ready`, `rows_rendered`, `selection_changed`, `mouse_click`, `action_requested`, `quit_requested`, background task lifecycle, terminal cleanup, exit).
- The observer does not mutate UI state, inject input, bypass actions, or grant authority; real input remains PTY-only.

Scenarios added:
- Panel mouse selection regression: waits for rendered rows, sends an SGR mouse click through PTY, asserts selection changed, and asserts no panel action was dispatched.
- Panel quit latency regression: waits for Panel ready plus background-task barrier, sends Ctrl+C through PTY, asserts clean exit within the threshold, and verifies the quit event.

Files changed:
- `Cargo.toml`, `Cargo.lock`
- `crates/tui/src/lib.rs`
- `crates/tui/src/multi_pod.rs`
- `crates/tui/src/e2e_observer.rs`
- `tests/e2e/Cargo.toml`
- `tests/e2e/src/lib.rs`
- `tests/e2e/tests/panel.rs`

Validation:
- `cargo build -p yoi` — passed.
- `YOI_E2E_BIN=/home/hare/Projects/yoi/.worktree/e2e-harness/target/debug/yoi cargo test -p yoi-e2e --features e2e --test panel -- --nocapture` — passed (2 tests).
- `cargo fmt --check` — passed.
- `cargo check -p tui --all-targets` — passed.
- `cargo check -p yoi --all-targets` — passed.
- `cargo check -p yoi-e2e --all-targets --features e2e` — passed.
- `git diff --check` — passed.

Remaining gaps / risks:
- The first slice is Unix PTY-based; cross-platform PTY support is not implemented.
- The screen artifact is currently raw PTY output rather than a parsed terminal snapshot.
- Fixture setup blocks Companion/Orchestrator role auto-spawn by placing intentionally invalid isolated test pod metadata in the fixture data dir; this avoids leaking live background Pods but should be replaced by a cleaner fixture strategy if the Panel gains an explicit test fixture mode.


---

<!-- event: implementation_report author: hare at: 2026-06-13T15:00:29Z -->

## Implementation report

Follow-up implementation update for review-required changes:

Mouse E2E false-positive prevention:
- `PanelHarness` now tracks the PTY output for terminal mouse mode enable/disable escape sequences.
- It requires both normal mouse tracking (`ESC[?1000h`, with disable `ESC[?1000l`) and SGR mouse mode (`ESC[?1006h`, with disable `ESC[?1006l`) to have been observed as currently enabled.
- Added `PanelHarness::expect_mouse_capture_enabled()` and the Panel mouse E2E calls it before clicking.
- `PanelHarness::click(...)` also fails with `MouseCaptureNotEnabled` if mouse capture has not been observed, so raw SGR mouse injection alone cannot make the test pass.

Production binary contamination reduction:
- Added explicit feature gates:
  - `tui/e2e-test`
  - `yoi/e2e-test = ["tui/e2e-test"]`
- The `YOI_TUI_TEST_EVENTS` JSONL event path and the background-task hold seam are compiled only under `tui/e2e-test`.
- Normal builds still type-check a no-op facade so call sites stay simple, but env-var observability/hold behavior is compiled out unless the feature is enabled.
- E2E binary build command is now: `cargo build -p yoi --features e2e-test`.

Quit latency barrier strengthening:
- Added a feature-gated test-only seam `YOI_TUI_TEST_HOLD_BACKGROUND_TASK=reload` that holds the Panel reload task pending in the E2E binary until quit aborts it.
- Added `PanelHarness::expect_background_task_pending("reload")`, which verifies that a `background_task_started` event exists and no matching `background_task_finished` or `background_task_aborted` event has superseded it.
- The quit latency test now uses the hold seam and asserts pending reload work immediately before sending Ctrl+C through the PTY.

Harness/artifact adjustment:
- E2E fixture artifacts now persist under workspace `target/e2e-artifacts/...` instead of package-local temporary dirs, so failure artifacts remain inspectable.

Validation after update:
- `cargo fmt --check` — passed.
- `cargo build -p yoi --features e2e-test` — passed.
- `YOI_E2E_BIN=/home/hare/Projects/yoi/.worktree/e2e-harness/target/debug/yoi cargo test -p yoi-e2e --features e2e --test panel -- --nocapture` — passed (2 tests).
- `git diff --check` — passed.
- `cargo check -p tui --all-targets` — passed.
- `cargo check -p yoi --all-targets` — passed.
- `cargo check -p tui --all-targets --features e2e-test` — passed.
- `cargo check -p yoi --all-targets --features e2e-test` — passed.
- `cargo check -p yoi-e2e --all-targets --features e2e` — passed.

Remaining gaps / risks:
- The E2E harness remains Unix PTY based.
- The screen artifact remains raw PTY output, not a parsed terminal snapshot.
- The test-only pending-task hold intentionally changes timing only for the `e2e-test` binary; it is not compiled into normal builds.


---

<!-- event: review author: hare at: 2026-06-13T15:05:52Z status: request_changes -->

## Review: request changes

Request changes.

Evidence reviewed:
- Inspected Ticket record and `git diff 134e8b8b..HEAD` for commits `96561897` and `10a1c383`.
- `tests/e2e` provides a credible first declarative harness (`PanelHarness::spawn`, `wait_for`, `wait_for_rows`, `click`, `press`, `expect_selection`, `expect_exit_within`, artifacts/metadata/input/output/event logs). This is not merely a fixed-sleep shell script.
- Mouse-selection scenario waits for rendered rows, verifies both normal mouse and SGR mouse capture before `click`, sends the click through PTY bytes, waits for `selection_changed`, and asserts no `action_requested` dispatch.
- Quit-latency scenario creates a real feature-gated background-task hold barrier, waits until the task is actually waiting before sending Ctrl+C through the PTY, and measures bounded exit latency.
- `yoi-e2e` is opt-in via package feature/test `required-features = ["e2e"]`; e2e tests are outside default members. `YOI_TUI_TEST_EVENTS` and `YOI_TUI_TEST_HOLD_BACKGROUND_TASK` env behavior is behind `tui/e2e-test` / `yoi/e2e-test` feature gates, and the hook is observability-only.

Required change:
- The normal production build still contains/evaluates too much e2e harness glue. In non-`e2e-test` builds, `crates/tui/src/e2e_observer.rs` exposes no-op `emit`/hold functions, but call sites still execute test-specific data construction. In particular `App::emit_rows_rendered` and its panel row key/rect DTOs are compiled unconditionally and `app.emit_rows_rendered()` is called from the panel render path, causing row snapshots to be built every draw even though emission is a no-op. Selection/action/quit call sites also construct `serde_json::json!` payloads before the no-op facade. This violates the recorded boundary that production binaries should not contain harness logic and production-side hooks must be feature-gated/compiled out for normal builds.
  - Please cfg-gate the call sites/helpers/DTOs, or use a lazy cfg-gated macro/helper so normal builds do not evaluate or retain e2e event payload construction. A tiny compile-only facade is acceptable only if it does not execute or allocate e2e-specific work and does not keep harness DTO logic in the normal runtime path.

Validation run in `/home/hare/Projects/yoi/.worktree/e2e-harness`:
- `git diff --check 134e8b8b..HEAD` — passed.
- `cargo fmt --check` — passed.
- `cargo check -p tui --all-targets` — passed.
- `cargo check -p yoi --all-targets` — passed.
- `cargo build -p yoi --features e2e-test` — passed.
- `YOI_E2E_BIN=/home/hare/Projects/yoi/.worktree/e2e-harness/target/debug/yoi cargo test -p yoi-e2e --features e2e --test panel -- --nocapture` — passed.
- `cargo check -p tui --all-targets --features e2e-test` — passed.
- `cargo check -p yoi --all-targets --features e2e-test` — passed.
- `cargo check -p yoi-e2e --all-targets --features e2e` — passed.

No source changes were made during review.


---
