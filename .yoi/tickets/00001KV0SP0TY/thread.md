<!-- event: create author: "yoi ticket" at: 2026-06-13T15:30:22Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: decision author: hare at: 2026-06-13T16:08:36Z -->

## Decision

決定:
- feature-layer `HostAuthority` は残さず削除する。
- rename して診断用に温存する案も採用しない。`Authority` / `Grant` という語彙を `pod::feature` public API に残すと、Plugin/MCP permission layer と再び混同されるため。
- feature install report に必要な情報は ordinary diagnostics / skipped contribution reason として表現する。


---

<!-- event: intake_summary author: hare at: 2026-06-13T16:27:15Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-13T16:27:15Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: workspace-panel at: 2026-06-13T16:33:15Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `workspace-panel` が queued にしました。


---
