<!-- event: create author: orchestrator at: 2026-06-13T15:46:07Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: orchestrator at: 2026-06-13T15:46:19Z -->

## Intake summary

ユーザーが `cargo build` による最新 `yoi` binary 入手を E2E harness default にする方針を明示した。要件・受け入れ条件は、`YOI_E2E_BIN` override を残しつつ、通常 E2E 実行では harness が `cargo build -p yoi --features e2e-test --bin yoi` を実行し、生成 binary を直接 PTY spawn すること。

---

<!-- event: state_changed author: orchestrator at: 2026-06-13T15:46:19Z from: planning to: ready reason: user_authorized_followup_ready field: state -->

## State changed

Ticket planning が完了しました。state planning -> ready。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-13T15:46:29Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `yoi ticket` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-06-13T15:46:54Z -->

## Decision

Routing decision: implementation_ready

Reason:
- ユーザーが方針を明示した: `cargo run` を PTY の process-under-test にせず、E2E harness が `cargo build -p yoi --features e2e-test --bin yoi` を実行し、生成された binary を直接 spawn する。
- Ticket は `queued` で、要件・受け入れ条件は具体的。blocking relation はなく、既存 E2E harness の小さな follow-up として実装可能。
- 既存 production/non-production boundary、mouse capture check、quit pending barrier は維持すべき invariant として明記済み。

Evidence checked:
- Ticket body / relation to `00001KSKBP9YG`。
- Existing E2E harness code: `tests/e2e/src/lib.rs::yoi_binary()` currently prefers `YOI_E2E_BIN`, then infers `target/debug/yoi`, but does not build it。
- Orchestrator worktree is clean。

IntentPacket:

Intent:
- `cargo test -p yoi-e2e --features e2e ...` を任意タイミングで実行しても、harness が最新 source から E2E 用 `yoi` binary を build して直接 PTY spawn するようにする。

Binding decisions / invariants:
- `cargo run` を process-under-test にしない。
- `YOI_E2E_BIN` explicit override は残してよい。
- default path は harness 内 `cargo build -p yoi --features e2e-test --bin yoi` + direct binary spawn。
- production/non-production boundary と `e2e-test` feature gating を壊さない。
- mouse capture tracking / quit pending barrier を壊さない。

Requirements / acceptance criteria:
- 事前 manual build なしに `cargo test -p yoi-e2e --features e2e --test panel -- --nocapture` が pass する。
- `YOI_E2E_BIN=<path>` 指定時は override としてその path が使われる。
- Build command / binary path が artifact または diagnostics から分かる。
- `cargo fmt --check`、`git diff --check`、関連 checks が pass。

Implementation latitude:
- `OnceLock` 等で同一 test process 内の build を 1 回にまとめてよい。
- Workspace root detection は `CARGO_MANIFEST_DIR` parent traversal、`cargo metadata`、または robust helper のいずれでもよい。
- Cargo executable は `CARGO` env を尊重する。

Escalate if:
- workspace/package layout 上、E2E test process 内から cargo build するのが大きな cyclic/build-system 問題を起こす場合。
- Nix/package source filter 等の追加変更が必要になる場合。

---

<!-- event: state_changed author: orchestrator at: 2026-06-13T15:47:00Z from: queued to: inprogress reason: orchestrator_acceptance_user_requested_fix field: state -->

## State changed

Ticket evidence、existing E2E harness code map、Orchestrator worktree clean state を確認した。ユーザーが明示的に修正を依頼しており、blocking relation はない。実装 side effect の前に inprogress acceptance を記録する。

---

<!-- event: decision author: orchestrator at: 2026-06-13T15:49:47Z -->

## Decision

追加の critical gap: E2E subprocess environment が host credentials を継承し得る。

Current finding:
- `PanelHarness::spawn` と fixture setup の `run_yoi_capture` は `Command::new(binary)` に対して `env_clear()` を呼んでいない。
- `HOME` / `XDG_*` は fixture dir に差し替えているが、通常の process environment は継承される。
- そのため、現時点の Panel tests は LLM call を意図的に起こしていないものの、何らかの E2E path が provider/LLM call に到達した場合、host の `OPENAI_API_KEY` / `ANTHROPIC_API_KEY` / その他 provider credentials や secret-related env を見てしまう可能性がある。

Decision:
- E2E harness は default で host environment を継承してはならない。
- `Command` 起動時は `env_clear()` を使い、必要最小限の env（`HOME`, `XDG_*`, `TERM`, `PATH`/`CARGO` 等 build/runtime に必要なもの、`YOI_TUI_TEST_EVENTS`, `YOI_POD_RUNTIME_COMMAND`, explicit test env）だけを allowlist で渡す。
- Provider credentials / secret refs / network credentials は default deny。LLM/provider E2E を将来追加する場合は fixture provider / canned server / explicit test-only env で渡す。
- Panel/TUI E2E は real LLM call が発生しないことを前提にし、host auth への依存を禁止する。

Acceptance addition for `00001KV0TJVN5`:
- Existing subprocess launches (`PanelHarness::spawn`, `run_yoi_capture`, cargo build provider where appropriate) must not leak host provider credentials to the tested `yoi` process。
- At minimum, yoi subprocess env should be allowlisted and provider key names should be absent unless explicitly supplied by a future fixture provider test。

---

<!-- event: decision author: orchestrator at: 2026-06-13T15:53:07Z -->

## Decision

Environment-boundary reference added: `docs/development/environment.md` is binding context for this follow-up.

Relevant policy:
- Environment variables are a minimized runtime boundary.
- Provider credentials and runtime configuration should not depend on ambient process state.
- Tests should prefer typed fixtures/injection and mutate process environment only around thin env-reader behavior.
- `YOI_POD_RUNTIME_COMMAND` is a development-only executable-path override, not a general ambient configuration channel.

Implication for this Ticket:
- E2E harness must not let tested `yoi` subprocesses inherit host env by default.
- `PanelHarness::spawn` and fixture setup commands should use `env_clear()` plus an explicit allowlist.
- Provider/secret/network credential env must be absent unless a future provider-fixture E2E explicitly injects test-only values.
- Existing `YOI_*` test env should be limited to the feature-gated E2E observability/binary runtime paths and documented as test-only/dev-only surface, not normal runtime configuration.
- The cargo build provider should avoid putting Cargo itself in the measured PTY path; any build env inheritance must be considered separately from the tested `yoi` subprocess env boundary.

---

<!-- event: implementation_report author: hare at: 2026-06-13T15:54:18Z -->

## Implementation report

Implementation report for Ticket 00001KV0TJVN5

Files changed:
- `tests/e2e/src/lib.rs`
  - Added a cached e2e binary provider using `OnceLock`.
  - Preserves `YOI_E2E_BIN=<path>` as the explicit override and skips the default cargo build provider in that path.
  - Default path runs `${CARGO:-cargo} build -p yoi --features e2e-test --bin yoi` from the workspace root, then returns the direct `target/{profile}/yoi` binary path for PTY spawning.
  - Writes `target/e2e-artifacts/binary-provider.json` and emits diagnostics with provider, build command, and binary path.
  - Expanded command-failure diagnostics to include command args.
- `tests/e2e/tests/panel.rs`
  - Updated panel tests to use the fallible cached binary provider.

Validation:
- `cargo fmt --check` — passed.
- `git diff --check` — passed.
- `cargo check -p yoi-e2e --all-targets --features e2e` — passed.
- `unset YOI_E2E_BIN && cargo test -p yoi-e2e --features e2e --test panel -- --nocapture` — passed; default provider built the current `yoi` binary and PTY-spawned `target/debug/yoi`.
- `YOI_E2E_BIN=/home/hare/Projects/yoi/.worktree/e2e-binary-provider/target/debug/yoi cargo test -p yoi-e2e --features e2e --test panel -- --nocapture` — passed; override provider path used without invoking the default cargo-build provider.

Remaining gaps:
- None known.


---

<!-- event: implementation_report author: hare at: 2026-06-13T16:01:51Z -->

## Implementation report

Implementation report for Ticket 00001KV0TJVN5

Files changed:
- `tests/e2e/src/lib.rs`
  - Added a cached e2e binary provider using `OnceLock`.
  - Preserves `YOI_E2E_BIN=<path>` as the explicit override and skips the default cargo build provider in that path.
  - Default path runs `${CARGO:-cargo} build -p yoi --features e2e-test --bin yoi` from the workspace root, then returns the direct `target/{profile}/yoi` binary path for PTY spawning.
  - Writes `target/e2e-artifacts/binary-provider.json` and emits diagnostics with provider, build command, binary path, and tested-subprocess env policy.
  - Expanded command-failure diagnostics to include command args.
  - Follow-up: isolated tested `yoi` subprocess environments in both `PanelHarness::spawn` and fixture setup `run_yoi_capture` with `env_clear()` plus explicit allowlists only.
  - Follow-up: recorded env policy in `run.json`, `binary-provider.json`, and per-fixture `fixture-commands.jsonl` artifacts.
  - Follow-up: added a regression assertion that tested-subprocess policies use `env_clear`, do not allow `PATH`, and default-deny provider credentials (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `GEMINI_API_KEY`) and secret-like patterns.
  - Follow-up: relative `YOI_E2E_BIN` values are resolved against the workspace root and must exist, so tested subprocess launch does not rely on `PATH` lookup.
- `tests/e2e/tests/panel.rs`
  - Updated panel tests to use the fallible cached binary provider.

Env isolation policy:
- Cargo build provider remains a build-tool command and is not treated as the tested `yoi` subprocess.
- Tested `yoi` fixture setup commands receive only: `HOME`, `XDG_DATA_HOME`, `XDG_STATE_HOME`, `XDG_CONFIG_HOME`, `YOI_POD_RUNTIME_COMMAND`.
- Tested `yoi panel` commands receive only: fixture `HOME`, `XDG_DATA_HOME`, `XDG_STATE_HOME`, `XDG_CONFIG_HOME`, `TERM`, `YOI_TUI_TEST_EVENTS`, `YOI_POD_RUNTIME_COMMAND`, and `YOI_TUI_TEST_HOLD_BACKGROUND_TASK` when used.
- `PATH` is intentionally not passed to tested `yoi` subprocesses; the harness launches the already-resolved binary path directly.
- Host provider credentials / token / secret-like environment variables are default-denied. Future provider/LLM E2E should use fixture providers, canned servers, or explicit test env instead of inheriting host credentials.

Validation:
- `cargo fmt --check` — passed.
- `git diff --check` — passed.
- `cargo check -p yoi-e2e --all-targets --features e2e` — passed.
- `cargo test -p yoi-e2e --features e2e tested_yoi_env_policy_is_env_clear_allowlist -- --nocapture` — passed.
- `unset YOI_E2E_BIN && OPENAI_API_KEY=host-secret ANTHROPIC_API_KEY=host-secret GEMINI_API_KEY=host-secret cargo test -p yoi-e2e --features e2e --test panel -- --nocapture` — passed; default provider built the current `yoi` binary and tested `yoi` subprocesses used isolated env policy artifacts. Host provider env was present for the harness but is not inherited by tested `yoi` subprocesses because `env_clear()` is applied before the allowlist.
- `YOI_E2E_BIN=/home/hare/Projects/yoi/.worktree/e2e-binary-provider/target/debug/yoi OPENAI_API_KEY=host-secret ANTHROPIC_API_KEY=host-secret GEMINI_API_KEY=host-secret cargo test -p yoi-e2e --features e2e --test panel -- --nocapture` — passed; override provider path used without invoking the default cargo-build provider, and tested `yoi` subprocesses still used isolated env policy.

Remaining gaps:
- None known.


---

<!-- event: review author: hare at: 2026-06-13T16:06:53Z status: approve -->

## Review: approve

## Review: approve

Decision: approve for Ticket `00001KV0TJVN5`.

Evidence reviewed:
- Ticket intent/acceptance criteria require default E2E setup to build `yoi` with `cargo build -p yoi --features e2e-test --bin yoi`, then direct-spawn the produced binary, while preserving `YOI_E2E_BIN` override and existing panel E2E behavior.
- `tests/e2e/src/lib.rs` now resolves `yoi_binary()` through a `OnceLock`-cached `BinaryProviderInfo`. The default path runs `${CARGO:-cargo} build -p yoi --features e2e-test --bin yoi` from the workspace root and returns `target/{debug|release}/yoi`; the override path validates and uses `YOI_E2E_BIN` without invoking the cargo-build provider.
- PTY execution remains `Command::new(&config.binary).arg("panel")`; `cargo run` is not in the process-under-test path.
- `PanelHarness::spawn` and fixture `run_yoi_capture` both call `env_clear()` and then set only explicit fixture/test variables. `PATH` and provider credentials are not allowlisted. `YOI_POD_RUNTIME_COMMAND` is set to the resolved binary path, so tested subprocesses do not need host `PATH`.
- Diagnostics/artifacts include provider/build/env policy in `target/e2e-artifacts/binary-provider.json`, panel `run.json`, and fixture `fixture-commands.jsonl`.
- Existing mouse-capture guard (`expect_mouse_capture_enabled` / SGR 1000+1006 tracking), background-task quit barrier assertions, and `e2e-test` production boundary code were not weakened by this diff.

Validation:
- Reviewer reran `git diff --check a4df9754..HEAD` — passed.
- Reviewer reran `cargo test -p yoi-e2e --features e2e tested_yoi_env_policy_is_env_clear_allowlist -- --nocapture` — passed.
- Also accepted Orchestrator-reported full validation, including fmt/check, `cargo check -p yoi-e2e --all-targets --features e2e`, default panel E2E with host provider env present, and `YOI_E2E_BIN` override panel E2E with host provider env present — all reported passed.

Risks / follow-up:
- No blocking issues found. The cargo build provider intentionally still uses build-tool environment; tested `yoi` subprocesses are isolated.


---

<!-- event: implementation_report author: orchestrator at: 2026-06-13T16:09:20Z -->

## Implementation report

Integration outcome: merged and validated.

- Implementation branch: `ticket-00001KV0TJVN5-e2e-binary-provider`
- Implementation commits: `13d00530 test: build e2e yoi binary provider`, `47efeb01 test: isolate e2e yoi subprocess env`
- Review commit/event: `7e24a8df ticket: approve e2e binary provider`
- Orchestrator merge commit: `8abc2b7f merge: e2e binary provider`

Implemented:
- Default E2E binary provider now runs `${CARGO:-cargo} build -p yoi --features e2e-test --bin yoi` from workspace root, caches the result, and direct-spawns the produced `target/{profile}/yoi` binary for PTY tests。
- `YOI_E2E_BIN=<path>` remains an explicit override and skips the default cargo-build provider。
- `cargo run` is not used as process-under-test; Cargo is not in the PTY/signal/quit-latency measured path。
- Tested `yoi` subprocesses (`PanelHarness::spawn` and fixture setup `run_yoi_capture`) now use `env_clear()` plus explicit allowlists only。
- Host provider credentials / token / secret-like env are default-denied for tested `yoi` subprocesses。
- Artifacts include binary provider/build command/binary path and tested subprocess env policy。

Orchestrator validation after merge:
- `cargo fmt --check`: PASS
- `git diff --check`: PASS
- `cargo check -p yoi-e2e --all-targets --features e2e`: PASS
- `cargo test -p yoi-e2e --features e2e tested_yoi_env_policy_is_env_clear_allowlist -- --nocapture`: PASS
- `unset YOI_E2E_BIN && OPENAI_API_KEY=host-secret ANTHROPIC_API_KEY=host-secret GEMINI_API_KEY=host-secret cargo test -p yoi-e2e --features e2e --test panel -- --nocapture`: PASS（default cargo-build provider used; 2 panel E2E tests passed）
- `YOI_E2E_BIN=/home/hare/Projects/yoi/.worktree/orchestration/yoi-orchestrator/target/debug/yoi OPENAI_API_KEY=host-secret ANTHROPIC_API_KEY=host-secret GEMINI_API_KEY=host-secret cargo test -p yoi-e2e --features e2e --test panel -- --nocapture`: PASS（override provider used; 2 panel E2E tests passed）

Residual note:
- Cargo build provider intentionally uses build-tool environment; only tested `yoi` subprocesses are env-isolated. Future LLM/provider E2E should use fixture providers/canned servers/explicit test env, not host credentials。

Next:
- Mark Ticket `done` and clean up child coder/reviewer Pods plus implementation worktree/branch. Closure remains separate。

---

<!-- event: state_changed author: orchestrator at: 2026-06-13T16:09:29Z from: inprogress to: done reason: merged_and_validated field: state -->

## State changed

E2E binary provider follow-up was reviewed, approved, merged into the Orchestrator branch as `8abc2b7f`, and validated in the Orchestrator worktree. Default E2E runs now build the current `yoi` binary before direct PTY spawn, `YOI_E2E_BIN` override remains available, and tested `yoi` subprocesses are isolated with `env_clear()` plus allowlist so host provider credentials are not inherited. Ticket implementation work is done; closure remains separate.

---

<!-- event: state_changed author: hare at: 2026-06-13T16:34:06Z from: done to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-06-13T16:34:06Z status: closed -->

## 完了

Closed after prior done-state completion.

---

<!-- event: comment author: orchestrator at: 2026-06-13T16:53:48Z -->

## Comment

Post-merge concern from user: E2E isolation may still have leaked or left visible Pod/worktree artifacts.

Observed concern:
- User reports Pods named `yoi-orchestrator-orchestrator` and `workspace-orchestrator` appeared.

Current assessment:
- Earlier E2E runs before the env isolation follow-up inherited host environment, including likely `XDG_RUNTIME_DIR`, so `yoi panel` could observe the host/global runtime Pod registry under `/run/user/...` even though `HOME`/`XDG_DATA_HOME` were fixture paths。
- The fixture also intentionally writes blocking Pod metadata for `workspace` and `workspace-orchestrator` under fixture `XDG_DATA_HOME` to drive panel rows. That should be fixture-local, but if runtime/data isolation is wrong it can become visible outside the intended fixture。
- The later `env_clear()` + allowlist fix prevents host env credential leak and likely prevents inheriting `XDG_RUNTIME_DIR`, causing runtime fallback to fixture HOME; however, no explicit regression assertion currently proves that E2E cannot see/create global runtime Pod state or workspace-orchestrator worktrees。

Required follow-up direction:
- Add explicit runtime isolation to E2E (`XDG_RUNTIME_DIR` or equivalent controlled fixture runtime path, or an assertion that fallback runtime is fixture-local)。
- Add regression assertions/artifacts proving tested `yoi panel` sees only fixture Pod metadata/runtime state and does not observe host live Pods。
- Ensure E2E cleanup removes any fixture Pod metadata/runtime/worktree artifacts it creates。
- Investigate and clean any residual `yoi-orchestrator-orchestrator` / `workspace-orchestrator` artifacts only after confirming whether they are live Pods, fixture artifacts, or prior Panel-created worktrees。

---
