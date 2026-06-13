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
