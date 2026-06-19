<!-- event: create author: "yoi ticket" at: 2026-06-19T04:07:17Z -->

## 作成

LocalTicketBackend によって作成されました。

---

<!-- event: state_changed author: hare at: 2026-06-19T04:19:09Z from: inprogress to: closed reason: closed field: state -->

## State changed

Ticket を closed にしました。


---

<!-- event: close author: hare at: 2026-06-19T04:19:09Z status: closed -->

## 完了

Implemented and validated.

Changes:
- Reused the initial `load_pod_list` result for Companion and Orchestrator presence in `load_multi_pod_snapshot`, removing two duplicate full Pod status probes before the first dashboard rows render.
- Renamed the E2E source timings to `companion.presence.from_initial_list` and `orchestrator.presence.from_initial_list` so regressions show whether the initial list is reused.
- Changed Pod list startup summarization to avoid reading active session logs while building initial Pod rows. Stored metadata now uses a cheap active-segment marker and live-only rows keep existing minimal live/pending summaries.
- Preserved spawn/restore behavior after Companion/Orchestrator lifecycle changes; if lifecycle changes require reload, the existing reload path remains.

Live-path measurement in the current workspace:
- Before this fix: first non-empty Panel rows appeared at about 7967ms; `pod_metadata_status_probe.initial`, `companion.presence`, and `orchestrator.presence` were each about 2.5s.
- After removing duplicate probes only: first non-empty rows appeared at about 2964ms; duplicate presence probes dropped to 0ms but initial Pod metadata/status probe was still about 2386ms.
- After also removing session-log reads from the startup Pod summary path: first non-empty rows appeared at about 754ms; `pod_metadata_status_probe.initial` was about 138ms; total dashboard source breakdown was about 649ms.

Validation:
- cargo test -p tui pod_list --lib
- cargo test -p yoi-e2e --features e2e --test panel
- cargo check -p yoi-e2e -p yoi -p tui --features tui/e2e-test
- cargo build -p yoi
- cargo fmt --check
- git diff --check
- target/debug/yoi ticket doctor
- nix build .#yoi --no-link


---
