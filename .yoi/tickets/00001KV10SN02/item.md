---
title: 'E2E: close remaining critical-path gaps after panel harness'
state: 'inprogress'
created_at: '2026-06-13T17:34:41Z'
updated_at: '2026-06-14T05:33:43Z'
assignee: null
readiness: 'implementation_ready'
risk_flags: ['e2e', 'tui', 'pty', 'quit-latency', 'mouse-input', 'rewind']
queued_by: 'workspace-panel'
queued_at: '2026-06-13T17:51:04Z'
---

## Background

Before starting the `pod::feature` / MCP work, finish a small but useful real-process E2E safety net. This is **not** a request to build a broad E2E matrix. The goal is to cover the product-critical TUI paths and the regressions that recently escaped focused unit/integration tests.

Relevant completed / in-flight history from git log:

- `00001KSKBP9YG` / `96561897` / `10a1c383` / `bdc735b8`: opt-in PTY Panel E2E harness exists under `tests/e2e`.
- `00001KV0TJVN5` / `13d00530` / `47efeb01` / `7fe463af`: E2E builds the current `yoi` binary by default and isolates tested subprocess env/provider credentials.
- `00001KV0YK5S0` / `07e754ce` / `20184eeb` / `6aa7c650`: orchestration branch adds fixture-local tmp workspace/data/runtime isolation and cleanup. If not already merged into the implementation base, incorporate or depend on that work rather than reimplementing it.
- `00001KV0723PC` / `cfe411e5`: late Panel quit latency was fixed; current E2E already has `panel_ctrl_c_exits_promptly_after_background_barrier` and should preserve/strengthen it.
- `00001KV072V89` / `452c9df1`: Panel mouse click selection was implemented; current E2E already has `panel_mouse_click_selects_row_without_dispatching_action` and should preserve/strengthen it.
- `00001KV04NJ8D` / `949ceb5a`: rewind live refresh was fixed, but it does not yet have a real-process PTY regression test.

## Requirements

- Keep E2E opt-in.
  - Tests remain under `tests/e2e` and behind the existing `e2e` feature gate.
  - `cargo test --workspace` must not run these tests by default.
- Do not duplicate already-covered Panel cases.
  - Preserve the existing late-quit-latency E2E.
  - Preserve the existing mouse click-to-select E2E.
  - If the tmp runtime isolation branch is not in the implementation base, merge/incorporate it first or make this Ticket depend on it.
- Add only the missing critical-path coverage needed before larger feature/MCP work.
  - A minimal Panel critical path must continue to cover startup, fixture row rendering/selection, and normal quit.
  - Add a minimal single-Pod/TUI critical path only as needed to support rewind coverage; do not add real provider/network calls.
- Strengthen late quit latency E2E only where useful.
  - Keep the held-background-task / observable barrier approach.
  - Assert user quit is observed and process exit remains within the bounded threshold.
  - Avoid arbitrary sleeps when an observable event/barrier exists.
- Strengthen mouse E2E for the currently missing behavior.
  - Keep click-to-select without action dispatch.
  - Add wheel input coverage for the viewport/row list behavior that was regressed by disabling mouse capture.
  - Ensure the test/harness can detect reintroducing full drag-motion capture (`?1002h` / `?1003h`) where feasible from PTY output.
  - Rename harness wording if needed: the current implementation enables normal mouse tracking for wheel/click, not full capture.
- Add rewind UI E2E.
  - Drive a real single-Pod TUI or equivalent PTY surface with fixture history/canned state.
  - `Ctrl+R` opens the rewind picker.
  - Selecting a rewind target with `Enter` causes the live TUI view/composer/state to update without requiring `Esc`, restart, or restore.
  - Repeated `Enter` while a rewind is pending does not send multiple destructive rewind requests or produce delayed duplicate success notices.
- Keep tests deterministic and content-safe.
  - No real provider credentials.
  - No network-backed LLM calls.
  - Use fixtures, canned sessions, test-only provider/runtime controls, or other existing test hooks when needed.
  - Do not leak host secret-like environment variables into tested processes.

## Acceptance criteria

- The focused E2E command, e.g. `cargo test -p yoi-e2e --features e2e --test panel -- --nocapture` plus any new E2E test target, runs from a clean checkout after building the current `yoi` binary automatically.
- The current Panel smoke/click/quit tests still pass on the fixture-isolated harness.
- If fixture runtime isolation is part of this implementation base, after a passing run no test Pod/socket appears in the user's real `~/.yoi` or `/run/user/<uid>/yoi`.
- Late quit latency remains covered by an E2E that fails if quit waits for held background work past the configured threshold.
- Mouse E2E fails if click-to-select dispatches an action, if wheel input is ignored, or if full drag-motion mouse capture is reintroduced where the harness can observe it.
- Rewind UI E2E fails if rewind success only becomes visible after `Esc`, TUI restart, or Pod restore.
- Failure artifacts include enough PTY output/events/status information to debug timing and UI failures.
- Existing non-E2E tests remain unaffected by the opt-in E2E feature gate.
- Validation includes focused E2E commands, affected crate tests/checks, `cargo build -p yoi`, and `nix build .#yoi`.

## Non-goals

- Full provider/API matrix E2E.
- Real network-backed LLM calls.
- CI-default enablement of E2E.
- Exhaustive Ticket orchestration multi-agent workflow E2E.
- Plugin/MCP E2E coverage.
- Replacing focused unit/integration tests; this Ticket adds a small real-process safety net.

## Related work

- E2E harness first slice: `00001KSKBP9YG`
- E2E latest-binary build/env isolation: `00001KV0TJVN5`
- E2E tmp runtime/data/workspace isolation: `00001KV0YK5S0`
- Rewind UI regression: `00001KV04NJ8D`
- Panel quit latency regression: `00001KV0723PC`
- Workspace panel mouse selection: `00001KV072V89`
