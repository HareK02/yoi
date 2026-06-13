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
