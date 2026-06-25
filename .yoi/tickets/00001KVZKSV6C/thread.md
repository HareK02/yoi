<!-- event: create author: "yoi ticket" at: 2026-06-25T14:44:03Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: decision author: hare at: 2026-06-25T16:27:28Z -->

## Decision

Decision update: split Backend Runtime work into three implementation tickets.

1. 00001KVZKSV6C Backend RuntimeRegistryの基盤をworker-runtime向けに整理する
   - Registry identity/projection/error boundary only.
   - No embedded Runtime handle implementation.
   - No remote Runtime client implementation.
2. 00001KVZSGT0Q Backend RuntimeRegistryにembedded worker-runtimeを接続する
   - Direct lib-call integration for backend-internal Runtime.
3. 00001KVZSGT14 Backend RuntimeRegistryにremote worker-runtime processを接続する
   - HTTP/event-stream client integration for remote Runtime processes.

Companion Web Console depends on embedded integration, not remote integration.


---

<!-- event: intake_summary author: hare at: 2026-06-25T16:30:31Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-25T16:30:31Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---
