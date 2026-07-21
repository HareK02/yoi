<!-- event: create author: "yoi ticket" at: 2026-07-18T02:39:04Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: intake_summary author: hare at: 2026-07-18T02:39:40Z -->

## Intake summary

Marked ready by `yoi ticket state`.

---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-18T02:39:40Z from: planning to: ready reason: cli_state field: state -->

## State changed

Marked ready by `yoi ticket state`.


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-18T02:39:40Z from: ready to: queued reason: queued field: state -->

## State changed

Ticket を `yoi ticket` が queued にしました。


---

<!-- event: state_changed author: "yoi ticket" at: 2026-07-18T02:39:40Z from: queued to: inprogress reason: cli_state field: state -->

## State changed

State changed to `inprogress`.


---

<!-- event: plan author: hare at: 2026-07-18T02:39:40Z -->

## Plan

Delegate implementation/recon to existing backend runtime Worker `arc/3`.

Requested focus:
- Surface the TUI-as-backend-runtime-client worker list route first.
- Inspect current TUI/backend runtime client implementation and make the smallest implementation that exposes worker list from backend/runtime authority.
- Include a staging/extract implementation check: verify the recent session-explore extract worker and flat staging writer still work through the runtime/embedded workspace abstraction; record findings and fix only concrete breakage.
- Leave attach/read/transcript UX as follow-up if it is larger than worker list.


---

<!-- event: decision author: hare at: 2026-07-18T02:46:09Z -->

## Decision

Paused before delegating work to Worker `arc/3`.

Reason:
- The current live Yoi session/runtime appears to be an older version that can emit obsolete/noisy memory staging records.
- Existing `.yoi/memory/_staging/*.json` from this session was quarantined without reading contents.
- No implementation prompt was sent to Worker `arc/3`; the Ticket is only prepared for continuation after restarting into the current build/runtime.

Resume condition:
- Restart with the current Yoi build/runtime/session.
- Confirm `.yoi/memory/_staging/` is clean or only contains new-format records.
- Then move this Ticket back through ready/queued/inprogress and delegate/implement.


---

<!-- event: implementation_report author: assistant at: 2026-07-21T09:12:45Z -->

## Implementation report

UI consistency follow-up:

- Committed `4a08b69b fix: align backend worker picker style`.
- Reworked `backend_worker_picker` to match the existing resume picker design rules:
  - inline viewport via `TerminalOptions { viewport: Viewport::Inline(...) }`
  - borderless one-line title / one row per worker / one-line hint layout
  - `▶` selection marker and Cyan primary identity styling
  - DarkGray metadata / preview styling and Green/Yellow/Red state badges
  - polling event loop with `↑/↓`, `j/k`, `Enter`, `Esc`, `Ctrl-C`
  - close-view handling matching the resume picker to avoid overwriting lower rows
- Removed the initial fullscreen bordered `List`/details-panel layout.

Verification:
- `nix develop -c cargo fmt`
- `nix develop -c cargo check -p tui`
- `nix develop -c cargo test -p tui backend_worker_picker --lib`
- `git diff --check`

---

<!-- event: state_changed author: hare at: 2026-07-21T09:16:15Z from: inprogress to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-07-21T09:16:15Z status: closed -->

## 完了

Backend/runtime authoritative worker list is now surfaced in the TUI.

Completed scope:
- Added backend runtime worker list DTO/API to the client crate.
- Added `yoi workers` and Backend runtime picker launch routing.
- Added client-side Backend URL resolution from explicit `--backend` or `$XDG_CONFIG_HOME/yoi/client.toml`, with workspace id read from explicit `--workspace-id` or `<workspace>/.yoi/workspace.toml`.
- Added TUI Backend worker picker using Backend `WorkerSummary` as authority and displaying runtime id, worker id, label/profile, state, and working-directory summary.
- Fixed missing GET routes for runtime worker list endpoints.
- Aligned the Backend worker picker styling/controls with the existing inline resume picker.
- Fixed the `nix develop` shell hook syntax error in `devshell.nix`.

Validation performed under `nix develop` included targeted cargo check/test/fmt and `git diff --check`. Manual TUI verification was also reported successful by the user.

---
