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
