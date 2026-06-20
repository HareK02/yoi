<!-- event: create author: ticket-intake at: 2026-06-20T10:46:48Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: ticket-intake at: 2026-06-20T10:46:54Z -->

## Intake summary

ユーザー要望を調査 Ticket ではなく concrete implementation Ticket として作成した。調査済み結論に基づき、`WebFetch` が `application/pdf` を `pdf-extract` で page-delimited Markdown-ish text として返せるようにする。Poppler/Pdfium/subprocess/OCR/semantic Markdown 化は非ゴール。既存 WebFetch safety bounds と HTML/text behavior は維持する。

---

<!-- event: state_changed author: ticket-intake at: 2026-06-20T10:46:54Z from: planning to: ready reason: implementation_ready field: state -->

## State changed

Intake 済み。Orchestrator は implementation routing として扱える。実装 side effect / worktree 作成 / coder 起動はここでは行っていない。

---
