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
