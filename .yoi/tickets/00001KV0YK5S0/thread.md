<!-- event: create author: orchestrator at: 2026-06-13T16:56:11Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: orchestrator at: 2026-06-13T16:56:22Z -->

## Intake summary

ユーザーが E2E を完全に clean な tmp environment で実行し、毎回 cleanup することを明示した。要件は workspace/HOME/XDG/runtime/artifacts の fixture 分離、host Pod/runtime registry 非参照、fixture-created Pod metadata の fixture-local 化、成功/失敗時 cleanup と artifact 記録。

---

<!-- event: state_changed author: orchestrator at: 2026-06-13T16:56:22Z from: planning to: ready reason: user_authorized_clean_tmp_e2e_isolation field: state -->

## State changed

Ticket planning が完了しました。state planning -> ready。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-13T16:56:31Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `yoi ticket` が queued にしました。


---

<!-- event: decision author: orchestrator at: 2026-06-13T16:56:50Z -->

## Decision

Routing decision: implementation_ready

Reason:
- ユーザーが E2E harness の clean tmp environment / cleanup を明示した。
- Ticket は `queued` で、要件・受け入れ条件は具体的。既存 `00001KSKBP9YG` / `00001KV0TJVN5` の follow-up で、主な変更面は `tests/e2e` に bounded される。
- Blocking relation はなく、既存 queued Tickets とは変更面が異なる。Orchestrator worktree は clean。
- Risk は test/runtime isolation だが、binding invariants（host runtime registry 非参照、fixture-local Pod metadata、cleanup artifact 記録）が明確なため実装可能。

Evidence checked:
- 新規 Ticket body / related records。
- `00001KV0TJVN5` の post-merge concern: host/global runtime registry を見た可能性、fixture `workspace-orchestrator` metadata、runtime isolation assertion 不足。
- Existing E2E code map: `tests/e2e/src/lib.rs` の fixture setup / `PanelHarness::spawn` / `run_yoi_capture` / artifact handling。
- Orchestrator workspace state: clean。

IntentPacket:

Intent:
- E2E harness が毎回 clean な tmp workspace/data/runtime environment で実行され、host Pod/runtime registry を見ず、成功・失敗に関係なく fixture temp を cleanup するようにする。

Binding decisions / invariants:
- Tested `yoi` process は host runtime/data/session/worktree/Pod registry を見ない。
- Fixture-created Pods (`workspace`, `workspace-orchestrator` 等) は fixture-local。
- Failure diagnostics は `target/e2e-artifacts` にコピーして残し、fixture temp root は cleanup する。
- Existing binary provider、env credential isolation、mouse capture guard、quit pending barrier を壊さない。
- Root/original workspace では作業しない。

Requirements / acceptance criteria:
- `cargo test -p yoi-e2e --features e2e --test panel -- --nocapture` が clean tmp env を使い、終了後に fixture temp root を残さない。
- Artifact に fixture workspace/data/runtime paths、cleanup result、env/runtime policy が残る。
- Test/assertion で Panel が host live Pods / host runtime registry を見ないことを確認する。
- Host `XDG_RUNTIME_DIR` 等が設定されていても tested `yoi` は fixture runtime だけを見る。
- `cargo fmt --check` / `git diff --check` / relevant checks / E2E tests が pass。

Implementation latitude:
- `tempfile::TempDir` / custom guard / artifact snapshot strategy の具体設計は Coder が選んでよい。
- Runtime dir は explicit `XDG_RUNTIME_DIR` fixture path でも、Yoi-specific fixture runtime env でもよいが、host fallback を使わないことを artifact/assertion で示す。
- Failure artifact は fixture tree 全体ではなく必要 subset copy でよい。

Escalate if:
- Yoi runtime path discovery に explicit test runtime dir を渡す public/production surface 変更が必要になる場合。
- Cleanup と artifact preservation が両立しない design constraint がある場合。

---

<!-- event: state_changed author: orchestrator at: 2026-06-13T16:56:58Z from: queued to: inprogress reason: orchestrator_acceptance_user_requested_clean_tmp_isolation field: state -->

## State changed

Ticket evidence、related E2E tickets、existing code map、Orchestrator worktree clean state を確認した。ユーザーが明示的に修正を依頼しており、blocking relation はない。実装 side effect の前に inprogress acceptance を記録する。

---

<!-- event: implementation_report author: hare at: 2026-06-13T17:06:29Z -->

## Implementation report

Implemented scoped E2E tmp/runtime isolation for Ticket 00001KV0YK5S0.

Files changed:
- `tests/e2e/src/lib.rs`
  - `FixtureWorkspace` now owns a `TempDir` under `target/e2e-tmp` and writes persistent scenario artifacts under `target/e2e-artifacts/<run-id>`.
  - Each fixture creates isolated workspace, HOME, XDG data/state/config/runtime dirs and passes `XDG_RUNTIME_DIR` explicitly to all tested `yoi` subprocesses under `env_clear`.
  - Fixture metadata records fixture root, workspace/home/XDG paths, runtime policy, env policy, and cleanup result.
  - Cleanup snapshots the fixture tree into persistent artifacts, closes/removes the temp root, writes `cleanup.json`, and reports cleanup failure.
- `tests/e2e/tests/panel.rs`
  - Existing two Panel E2E tests now assert fixture path isolation, absence of host/runtime Pod marker leakage in rendered rows, and explicit fixture cleanup/removal after each scenario.

Isolation/cleanup policy:
- Tested `yoi` never inherits host env wholesale; fixture setup and Panel subprocesses use `env_clear` with an allowlist that includes only fixture HOME/XDG data/state/config/runtime plus test/runtime-specific variables.
- `XDG_RUNTIME_DIR` is always a fixture-local runtime dir under the temporary fixture root.
- Fixture-local Pod metadata markers (`workspace`, `workspace-orchestrator`) are created only inside fixture data home and panel rows assert no host/fixture runtime markers leak.
- Artifacts persist outside the temp root in `target/e2e-artifacts`; cleanup snapshots selected fixture contents there before removing the temp root.

Validation:
- `cargo check -p yoi-e2e --all-targets --features e2e` — passed.
- `cargo test -p yoi-e2e --features e2e --test panel -- --nocapture` — passed (2 tests).
- `XDG_RUNTIME_DIR=/tmp/yoi-e2e-host-runtime-leak-test OPENAI_API_KEY=host-secret cargo test -p yoi-e2e --features e2e --test panel -- --nocapture` — passed (2 tests); artifacts show fixture `xdg_runtime_dir`, `host_xdg_runtime_dir_present: true`, and `cleanup_success: true`.
- `YOI_E2E_BIN=target/debug/yoi cargo test -p yoi-e2e --features e2e --test panel -- --nocapture` — passed (2 tests).
- `cargo fmt --check` — passed.
- `git diff --check` — passed.

Remaining gaps:
- No known gaps within the delegated scope.


---
