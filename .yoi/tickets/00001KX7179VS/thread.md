<!-- event: create author: "yoi ticket" at: 2026-07-10T22:08:58Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-07-10T22:09:20Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-10T22:09:20Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-10T22:09:21Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `yoi ticket` が queued にしました。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-10T22:09:21Z from: queued to: inprogress reason: cli_state field: state -->

## State changed

State changed to `inprogress`.


---

<!-- event: implementation_report author: hare at: 2026-07-10T22:16:58Z -->

## Implementation report

Implemented Workspace Console tool-call grouping.

- Tool lifecycle events now update one `Call · <tool>` console line keyed by call id.
- Args streaming, call completion, and tool result output are folded into that line instead of producing separate `tool call` / `tool call done` / `tool result` rows.
- Added generic tool rendering plus TUI-aligned special summaries for Read, Write, Edit, Glob, and Grep; Bash renders the command and output in the same Call block.
- In-flight tool snapshots now render as Call blocks as well.
- Added model tests for completed Bash call aggregation and streaming Read call updates.

Validation:
- cd web/workspace && deno task check
- cd web/workspace && deno task test
- git diff --check
- nix build .#yoi --no-link


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-10T22:16:58Z from: inprogress to: done reason: cli_state field: state -->

## State changed

State changed to `done`.


---

<!-- event: state_changed author: hare at: 2026-07-10T22:17:10Z from: done to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-07-10T22:17:10Z status: closed -->

## 完了

Implemented frontend Workspace Console tool call grouping. Tool call start/arg/done/result events now project into a single `Call · <tool>` block keyed by call id, with streaming state updates and folded result output. Added generic and TUI-aligned specialized summaries for core tools, including Bash command/output rendering. Validated with frontend check/test, diff check, and Nix package build.


---
