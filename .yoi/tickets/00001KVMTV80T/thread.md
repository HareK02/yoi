<!-- event: create author: "yoi ticket" at: 2026-06-21T10:15:30Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-06-21T10:16:04Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-21T10:16:04Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-21T10:16:04Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `yoi ticket` が queued にしました。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-21T10:16:04Z from: queued to: inprogress reason: cli_state field: state -->

## State changed

State changed to `inprogress`.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-06-21T10:41:20Z from: inprogress to: done reason: cli_state field: state -->

## State changed

State changed to `done`.


---

<!-- event: state_changed author: hare at: 2026-06-21T10:41:20Z from: done to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-06-21T10:41:20Z status: closed -->

## 完了

Added standalone yoi-workspace-server binary entrypoint with serve command and options for workspace/db/frontend/listen. Added yoi workspace serve launcher that resolves an external executable via YOI_WORKSPACE_SERVER_COMMAND or sibling yoi-workspace-server without linking the workspace-server crate into yoi. Updated package.nix to build/install both separate binaries. Validation passed: cargo fmt --check; cargo check -p yoi -p yoi-workspace-server; cargo test -p yoi workspace; cargo test -p yoi-workspace-server; cargo build -p yoi -p yoi-workspace-server; help/launch smoke tests; cargo tree dependency check; git diff --check; yoi ticket doctor; yoi objective doctor; nix build .#yoi --no-link.


---
