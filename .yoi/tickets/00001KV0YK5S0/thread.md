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
