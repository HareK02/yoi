<!-- event: create author: "yoi ticket" at: 2026-07-11T00:09:06Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-07-11T00:09:32Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-11T00:09:32Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-11T00:09:32Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `yoi ticket` が queued にしました。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-11T00:09:33Z from: queued to: inprogress reason: cli_state field: state -->

## State changed

State changed to `inprogress`.


---

<!-- event: implementation_report author: hare at: 2026-07-11T00:31:41Z -->

## Implementation report

Removed the backend-local worker observation cursor surface.

- Browser-facing worker observation envelopes no longer include `cursor`; they carry runtime/worker identity, runtime event id, and payload.
- Workspace-server observation proxy no longer owns cursor state, cursor validation, replay state, or store/open methods; it only maps upstream Runtime events to browser envelopes.
- Workspace-server WS attach ignores no cursor path because no cursor query is accepted/extracted.
- Backend Runtime client no longer sends cursor query params or deduplicates by backend cursor.
- Frontend Console no longer stores cursor/lastCursor and projects events by runtime event id.

Validation:
- cargo test -q -p yoi-workspace-server
- cargo test -q -p client
- cd web/workspace && deno task check
- cd web/workspace && deno task test
- cargo test -q
- git diff --check
- nix build .#yoi --no-link


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-11T00:31:41Z from: inprogress to: done reason: cli_state field: state -->

## State changed

State changed to `done`.


---

<!-- event: state_changed author: hare at: 2026-07-11T00:31:57Z from: done to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-07-11T00:31:57Z status: closed -->

## 完了

Removed browser-facing worker observation cursor support. Workspace Console and Backend Runtime clients now attach to Snapshot + live events without backend cursor query or backend-local cursor state. Event envelopes expose runtime event id for dedupe only.


---
